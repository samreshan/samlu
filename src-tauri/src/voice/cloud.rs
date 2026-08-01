//! OpenAI-compatible cloud speech and text transforms. Groq is the default
//! preset; custom providers can supply another compatible base URL.

use super::config::EndpointConfig;
use super::Mode;
use serde::Deserialize;
use std::sync::OnceLock;
use std::time::Duration;

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
) -> Result<String, String> {
    let part = reqwest::multipart::Part::bytes(wav_bytes)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|error| error.to_string())?;
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", endpoint.model.clone())
        .text("response_format", "json");

    let response = client()?
        .post(format!("{}/audio/transcriptions", endpoint.base_url))
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await
        .map_err(|error| {
            format!(
                "{} transcription request failed: {error}",
                endpoint.provider
            )
        })?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("transcription failed ({status}): {body}"));
    }
    response
        .json::<TranscriptionResponse>()
        .await
        .map(|response| response.text)
        .map_err(|error| format!("could not parse transcription response: {error}"))
}

pub async fn transform(
    text: &str,
    mode: Mode,
    endpoint: &EndpointConfig,
    api_key: &str,
) -> Result<String, String> {
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
    let response = client()?
        .post(format!("{}/chat/completions", endpoint.base_url))
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("{} transform request failed: {error}", endpoint.provider))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("prompt transform failed ({status}): {body}"));
    }
    response
        .json::<ChatResponse>()
        .await
        .map_err(|error| format!("could not parse transform response: {error}"))?
        .choices
        .into_iter()
        .next()
        .map(|choice| choice.message.content)
        .ok_or_else(|| "transform provider returned no choices".to_string())
}
