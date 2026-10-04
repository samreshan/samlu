//! Deepgram pre-recorded transcription (`POST /v1/listen`).

use super::{PreparedAudio, SttOptions};
use crate::voice::cloud::{self, ProviderError};
use crate::voice::config::DEEPGRAM_DEFAULT_MODEL;
use crate::voice::vocabulary;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;

const LISTEN_URL: &str = "https://api.deepgram.com/v1/listen";
/// Deepgram rejects key terms beyond 500 tokens per request.
const KEYTERM_CHARS: usize = 1_500;

#[derive(Deserialize)]
struct ListenResponse {
    results: ListenResults,
}

#[derive(Deserialize)]
struct ListenResults {
    channels: Vec<ListenChannel>,
}

#[derive(Deserialize)]
struct ListenChannel {
    alternatives: Vec<ListenAlternative>,
}

#[derive(Deserialize)]
struct ListenAlternative {
    transcript: String,
}

pub(super) fn build_request(
    client: &reqwest::Client,
    wav: Vec<u8>,
    opts: &SttOptions,
    api_key: &str,
) -> Result<reqwest::Request, reqwest::Error> {
    let model = match opts.settings.model.trim() {
        "" => DEEPGRAM_DEFAULT_MODEL,
        model => model,
    };
    let mut query: Vec<(&str, String)> = vec![
        ("model", model.to_string()),
        ("smart_format", "true".to_string()),
    ];
    match opts.language() {
        Some(language) => query.push(("language", language)),
        None => query.push(("detect_language", "true".to_string())),
    }
    if model.starts_with("nova-3") {
        for term in vocabulary::take_within(&opts.vocabulary, KEYTERM_CHARS) {
            query.push(("keyterm", term.to_string()));
        }
    }
    client
        .post(LISTEN_URL)
        .query(&query)
        .header(AUTHORIZATION, format!("Token {api_key}"))
        .header(CONTENT_TYPE, "audio/wav")
        .body(wav)
        .build()
}

pub(super) fn parse_transcript(body: &str) -> Result<String, String> {
    let response: ListenResponse = serde_json::from_str(body)
        .map_err(|error| format!("could not parse Deepgram response: {error}"))?;
    response
        .results
        .channels
        .into_iter()
        .next()
        .and_then(|channel| channel.alternatives.into_iter().next())
        .map(|alternative| alternative.transcript)
        .ok_or_else(|| "Deepgram returned no transcript.".to_string())
}

pub(super) async fn transcribe(
    audio: &PreparedAudio,
    opts: &SttOptions,
) -> Result<String, ProviderError> {
    let client = cloud::client().map_err(ProviderError::permanent)?;
    let request = build_request(client, audio.wav.clone(), opts, opts.api_key()?)
        .map_err(|error| ProviderError::permanent(error.to_string()))?;
    let response = client.execute(request).await.map_err(|error| {
        let error = error.without_url();
        let message = format!("Deepgram transcription request failed: {error}");
        ProviderError::from_send(error, message)
    })?;
    if !response.status().is_success() {
        return Err(ProviderError::from_response(response, "Deepgram transcription").await);
    }
    let body = response.text().await.map_err(|error| {
        ProviderError::permanent(format!("could not read Deepgram response: {error}"))
    })?;
    parse_transcript(&body).map_err(ProviderError::permanent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::config::{SttEngine, SttSettings};

    fn options(language: &str, vocabulary: &[&str]) -> SttOptions {
        SttOptions {
            settings: SttSettings {
                engine: SttEngine::Deepgram,
                preset: None,
                model: String::new(),
                base_url: None,
                language: language.to_string(),
            },
            vocabulary: vocabulary.iter().map(|term| term.to_string()).collect(),
            api_key: Some("dg-key".into()),
        }
    }

    fn query(request: &reqwest::Request) -> Vec<(String, String)> {
        request
            .url()
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect()
    }

    #[test]
    fn request_uses_nova3_detect_language_and_token_auth() {
        let request = build_request(
            &reqwest::Client::new(),
            vec![1, 2],
            &options("auto", &[]),
            "dg-key",
        )
        .unwrap();
        assert_eq!(request.url().path(), "/v1/listen");
        let pairs = query(&request);
        assert!(pairs.contains(&("model".into(), "nova-3".into())));
        assert!(pairs.contains(&("smart_format".into(), "true".into())));
        assert!(pairs.contains(&("detect_language".into(), "true".into())));
        assert_eq!(request.headers()["authorization"], "Token dg-key");
        assert_eq!(request.headers()["content-type"], "audio/wav");
    }

    #[test]
    fn request_sends_language_and_one_keyterm_per_term() {
        let request = build_request(
            &reqwest::Client::new(),
            vec![],
            &options("de", &["Samlu", "Claude Code"]),
            "k",
        )
        .unwrap();
        let pairs = query(&request);
        assert!(pairs.contains(&("language".into(), "de".into())));
        assert!(!pairs.iter().any(|(key, _)| key == "detect_language"));
        let keyterms: Vec<_> = pairs
            .iter()
            .filter(|(key, _)| key == "keyterm")
            .map(|(_, value)| value.as_str())
            .collect();
        assert_eq!(keyterms, vec!["Samlu", "Claude Code"]);
    }

    #[test]
    fn nova2_model_with_vocabulary_sends_no_keyterm() {
        let mut opts = options("en", &["Samlu", "Claude Code"]);
        opts.settings.model = "nova-2-general".to_string();
        let request = build_request(&reqwest::Client::new(), vec![], &opts, "k").unwrap();
        let pairs = query(&request);
        assert!(pairs.contains(&("model".into(), "nova-2-general".into())));
        assert!(!pairs.iter().any(|(key, _)| key == "keyterm"));
    }

    #[test]
    fn parses_first_alternative_transcript() {
        let body = r#"{"results":{"channels":[{"alternatives":[{"transcript":"hello world","confidence":0.99}]}]}}"#;
        assert_eq!(parse_transcript(body).unwrap(), "hello world");
        assert!(parse_transcript(r#"{"results":{"channels":[]}}"#).is_err());
    }
}
