//! OpenAI-compatible cloud speech and text transforms. Groq is the default
//! preset; custom providers can supply another compatible base URL.

use super::config::EndpointConfig;
use super::Mode;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::StatusCode;
use serde::Deserialize;
use std::future::Future;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub const MAX_ATTEMPTS: u32 = 3;
const BASE_BACKOFF: Duration = Duration::from_millis(700);
/// Automatic retries stop once this much time has passed, so a provider that
/// hangs (rather than failing fast) hands control back to the user instead of
/// stacking request timeouts. The recording is kept for a manual retry.
const RETRY_BUDGET: Duration = Duration::from_secs(20);
/// A longer server-requested wait usually means an exhausted quota, not a
/// blip; surface it rather than leave the user staring at a spinner.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(6);

/// A failed provider call, classified so transient failures are retried
/// automatically while configuration problems surface immediately.
#[derive(Debug)]
pub struct ProviderError {
    message: String,
    transient: bool,
    retry_after: Option<Duration>,
}

impl ProviderError {
    fn permanent(message: String) -> Self {
        Self {
            message,
            transient: false,
            retry_after: None,
        }
    }

    fn from_send(error: reqwest::Error, message: String) -> Self {
        Self {
            message,
            // Timeouts and connection failures are worth another attempt;
            // a request that could not even be built is not.
            transient: !error.is_builder(),
            retry_after: None,
        }
    }

    async fn from_response(response: reqwest::Response, context: &str) -> Self {
        let status = response.status();
        let retry_after = parse_retry_after(response.headers());
        let body = response.text().await.unwrap_or_default();
        Self {
            message: format!("{context} failed ({status}): {body}"),
            transient: is_transient_status(status),
            retry_after,
        }
    }

    fn retry_delay(&self, attempt: u32) -> Option<Duration> {
        if !self.transient {
            return None;
        }
        match self.retry_after {
            Some(after) if after > MAX_RETRY_AFTER => None,
            Some(after) => Some(after),
            None => Some(BASE_BACKOFF * 2_u32.pow(attempt.saturating_sub(1))),
        }
    }
}

fn is_transient_status(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

/// Only the delay-seconds form; providers in practice do not send HTTP dates.
fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    let seconds: f64 = headers
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    (seconds.is_finite() && seconds >= 0.0).then(|| Duration::from_secs_f64(seconds))
}

/// Runs `request` until it succeeds, fails permanently, or exhausts the
/// attempt and time budgets. `on_retry` receives the upcoming attempt number
/// so the capsule can show that Samlu is still working.
pub async fn with_retries<T, F, Fut>(
    mut request: F,
    mut on_retry: impl FnMut(u32),
) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
{
    let started = Instant::now();
    let mut attempt = 1;
    loop {
        let error = match request().await {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        let delay = error
            .retry_delay(attempt)
            .filter(|delay| attempt < MAX_ATTEMPTS && started.elapsed() + *delay < RETRY_BUDGET);
        let Some(delay) = delay else {
            return Err(error.message);
        };
        log::warn!(
            "[voice] attempt {attempt} failed; retrying in {delay:?}: {}",
            error.message
        );
        attempt += 1;
        on_retry(attempt);
        let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(delay)).await;
    }
}

#[derive(Deserialize)]
struct TranscriptionResponse {
    text: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatMessage {
    content: String,
}

fn client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(75))
                .build()
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| error.clone())
}

pub async fn transcribe(
    wav_bytes: Vec<u8>,
    endpoint: &EndpointConfig,
    api_key: &str,
) -> Result<String, ProviderError> {
    let part = reqwest::multipart::Part::bytes(wav_bytes)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|error| ProviderError::permanent(error.to_string()))?;
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", endpoint.model.clone())
        .text("response_format", "json");

    let response = client()
        .map_err(ProviderError::permanent)?
        .post(format!("{}/audio/transcriptions", endpoint.base_url))
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await
        .map_err(|error| {
            let message = format!(
                "{} transcription request failed: {error}",
                endpoint.provider
            );
            ProviderError::from_send(error, message)
        })?;
    if !response.status().is_success() {
        return Err(ProviderError::from_response(response, "transcription").await);
    }
    response
        .json::<TranscriptionResponse>()
        .await
        .map(|response| response.text)
        .map_err(|error| {
            ProviderError::permanent(format!("could not parse transcription response: {error}"))
        })
}

pub async fn transform(
    text: &str,
    mode: Mode,
    endpoint: &EndpointConfig,
    api_key: &str,
) -> Result<String, ProviderError> {
    let system_prompt = match mode {
        Mode::Normal => return Ok(text.to_string()),
        Mode::Summarize => {
            "Summarize the dictated speech concisely. Preserve decisions, technical details, \
             file names, commands, and constraints. Return only the summary."
        }
        Mode::Prompt => {
            "Rewrite the dictated speech as a detailed prompt for an AI coding agent. Preserve \
             all technical details and constraints. Organize the request into objective, context, \
             requirements, and acceptance criteria when those sections are supported by the \
             dictation. Do not invent requirements. Return only the prompt."
        }
    };
    let body = serde_json::json!({
        "model": endpoint.model,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": text}
        ],
        "temperature": 0.2
    });
    let response = client()
        .map_err(ProviderError::permanent)?
        .post(format!("{}/chat/completions", endpoint.base_url))
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|error| {
            let message = format!("{} transform request failed: {error}", endpoint.provider);
            ProviderError::from_send(error, message)
        })?;
    if !response.status().is_success() {
        return Err(ProviderError::from_response(response, "prompt transform").await);
    }
    let content = response
        .json::<ChatResponse>()
        .await
        .map_err(|error| {
            ProviderError::permanent(format!("could not parse transform response: {error}"))
        })?
        .choices
        .into_iter()
        .next()
        .map(|choice| choice.message.content)
        .ok_or_else(|| {
            ProviderError::permanent("transform provider returned no choices".to_string())
        })?;
    if content.trim().is_empty() {
        return Err(ProviderError::permanent(
            "transform provider returned an empty result".to_string(),
        ));
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn transient(retry_after: Option<Duration>) -> ProviderError {
        ProviderError {
            message: "provider unavailable".to_string(),
            transient: true,
            retry_after,
        }
    }

    #[test]
    fn rate_limits_and_server_errors_are_transient() {
        assert!(is_transient_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_transient_status(StatusCode::REQUEST_TIMEOUT));
        assert!(is_transient_status(StatusCode::BAD_GATEWAY));
        assert!(is_transient_status(StatusCode::SERVICE_UNAVAILABLE));
        assert!(!is_transient_status(StatusCode::UNAUTHORIZED));
        assert!(!is_transient_status(StatusCode::NOT_FOUND));
        assert!(!is_transient_status(StatusCode::PAYLOAD_TOO_LARGE));
    }

    #[test]
    fn retry_delay_backs_off_and_respects_server_requests() {
        assert_eq!(transient(None).retry_delay(1), Some(BASE_BACKOFF));
        assert_eq!(transient(None).retry_delay(2), Some(BASE_BACKOFF * 2));
        assert_eq!(
            transient(Some(Duration::from_secs(2))).retry_delay(1),
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            transient(Some(Duration::from_secs(60))).retry_delay(1),
            None
        );
        assert_eq!(
            ProviderError::permanent("bad key".into()).retry_delay(1),
            None
        );
    }

    #[test]
    fn parses_retry_after_seconds() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, "1.5".parse().unwrap());
        assert_eq!(
            parse_retry_after(&headers),
            Some(Duration::from_millis(1_500))
        );
        headers.insert(
            RETRY_AFTER,
            "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(parse_retry_after(&headers), None);
    }

    #[test]
    fn retries_transient_failures_until_success() {
        let calls = Cell::new(0);
        let mut retries = Vec::new();
        let result = tauri::async_runtime::block_on(with_retries(
            || {
                calls.set(calls.get() + 1);
                let outcome = if calls.get() < 3 {
                    Err(transient(Some(Duration::ZERO)))
                } else {
                    Ok("hello")
                };
                async move { outcome }
            },
            |attempt| retries.push(attempt),
        ));
        assert_eq!(result, Ok("hello"));
        assert_eq!(calls.get(), 3);
        assert_eq!(retries, vec![2, 3]);
    }

    #[test]
    fn gives_up_after_the_attempt_budget() {
        let calls = Cell::new(0);
        let result: Result<(), String> = tauri::async_runtime::block_on(with_retries(
            || {
                calls.set(calls.get() + 1);
                async { Err(transient(Some(Duration::ZERO))) }
            },
            |_| {},
        ));
        assert_eq!(result, Err("provider unavailable".to_string()));
        assert_eq!(calls.get(), MAX_ATTEMPTS);
    }

    #[test]
    fn permanent_failures_are_not_retried() {
        let calls = Cell::new(0);
        let result: Result<(), String> = tauri::async_runtime::block_on(with_retries(
            || {
                calls.set(calls.get() + 1);
                async { Err(ProviderError::permanent("invalid API key".into())) }
            },
            |_| {},
        ));
        assert_eq!(result, Err("invalid API key".to_string()));
        assert_eq!(calls.get(), 1);
    }
}
