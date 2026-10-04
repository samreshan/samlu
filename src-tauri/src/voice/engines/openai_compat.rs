//! OpenAI-compatible `/audio/transcriptions` (Groq, OpenAI, custom URLs).

use super::{PreparedAudio, SttOptions};
use crate::voice::cloud::{self, ProviderError};
use crate::voice::config::DEFAULT_BASE_URL;
use crate::voice::vocabulary;
use serde::Deserialize;

#[derive(Deserialize)]
struct TranscriptionResponse {
    text: String,
}

pub(super) fn form_fields(opts: &SttOptions) -> Vec<(&'static str, String)> {
    let mut fields = vec![
        ("model", opts.settings.model.clone()),
        ("response_format", "json".to_string()),
    ];
    if let Some(language) = opts.language() {
        fields.push(("language", language));
    }
    if let Some(prompt) = vocabulary::whisper_prompt(&opts.vocabulary) {
        fields.push(("prompt", prompt));
    }
    fields
}

pub(super) async fn transcribe(
    audio: &PreparedAudio,
    opts: &SttOptions,
) -> Result<String, ProviderError> {
    let base_url = opts
        .settings
        .base_url
        .as_deref()
        .unwrap_or(DEFAULT_BASE_URL);
    let provider = opts.settings.preset.as_deref().unwrap_or("custom");
    let part = reqwest::multipart::Part::bytes(audio.wav.clone())
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|error| ProviderError::permanent(error.to_string()))?;
    let mut form = reqwest::multipart::Form::new().part("file", part);
    for (name, value) in form_fields(opts) {
        form = form.text(name, value);
    }
    let response = cloud::client()
        .map_err(ProviderError::permanent)?
        .post(format!("{base_url}/audio/transcriptions"))
        .bearer_auth(opts.api_key()?)
        .multipart(form)
        .send()
        .await
        .map_err(|error| {
            let error = error.without_url();
            let message = format!("{provider} transcription request failed: {error}");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::config::SttSettings;

    fn options(language: &str, vocabulary: &[&str]) -> SttOptions {
        SttOptions {
            settings: SttSettings {
                language: language.to_string(),
                ..SttSettings::default()
            },
            vocabulary: vocabulary.iter().map(|term| term.to_string()).collect(),
            api_key: Some("key".into()),
        }
    }

    #[test]
    fn auto_language_without_vocabulary_sends_only_model() {
        let fields = form_fields(&options("auto", &[]));
        assert_eq!(
            fields,
            vec![
                ("model", "whisper-large-v3-turbo".to_string()),
                ("response_format", "json".to_string()),
            ]
        );
    }

    #[test]
    fn language_and_vocabulary_are_sent_as_fields() {
        let fields = form_fields(&options("de", &["Samlu"]));
        assert!(fields.contains(&("language", "de".to_string())));
        assert!(fields.contains(&("prompt", "Glossary: Samlu.".to_string())));
    }
}
