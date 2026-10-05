//! ElevenLabs Scribe (`POST /v1/speech-to-text`).

use super::{PreparedAudio, SttOptions};
use crate::voice::cloud::{self, ProviderError};
use crate::voice::config::ELEVENLABS_DEFAULT_MODEL;
use serde::Deserialize;

const STT_URL: &str = "https://api.elevenlabs.io/v1/speech-to-text";
/// ElevenLabs rejects key terms longer than five words.
const MAX_KEYTERM_WORDS: usize = 5;

#[derive(Deserialize)]
struct SpeechToTextResponse {
    text: String,
}

pub(super) fn form_fields(opts: &SttOptions) -> Vec<(&'static str, String)> {
    let model = match opts.settings.model.trim() {
        "" => ELEVENLABS_DEFAULT_MODEL,
        model => model,
    };
    let mut fields = vec![
        ("model_id", model.to_string()),
        ("tag_audio_events", "false".to_string()),
    ];
    if let Some(language) = opts.language() {
        fields.push(("language_code", language));
    }
    // Key terms are a Scribe v2 feature; earlier models reject the field.
    if model.starts_with("scribe_v2") {
        for term in &opts.vocabulary {
            if term.split_whitespace().count() <= MAX_KEYTERM_WORDS {
                fields.push(("keyterms", term.clone()));
            }
        }
    }
    fields
}

pub(super) async fn transcribe(
    audio: &PreparedAudio,
    opts: &SttOptions,
) -> Result<String, ProviderError> {
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
        .post(STT_URL)
        .header("xi-api-key", opts.api_key()?)
        .multipart(form)
        .send()
        .await
        .map_err(|error| {
            let error = error.without_url();
            let message = format!("ElevenLabs transcription request failed: {error}");
            ProviderError::from_send(error, message)
        })?;
    if !response.status().is_success() {
        return Err(ProviderError::from_response(response, "ElevenLabs transcription").await);
    }
    response
        .json::<SpeechToTextResponse>()
        .await
        .map(|response| response.text)
        .map_err(|error| {
            ProviderError::permanent(format!("could not parse ElevenLabs response: {error}"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::config::{SttEngine, SttSettings};

    fn options(model: &str, language: &str, vocabulary: &[&str]) -> SttOptions {
        SttOptions {
            settings: SttSettings {
                engine: SttEngine::Elevenlabs,
                preset: None,
                model: model.to_string(),
                base_url: None,
                language: language.to_string(),
            },
            vocabulary: vocabulary.iter().map(|term| term.to_string()).collect(),
            api_key: Some("xi".into()),
        }
    }

    #[test]
    fn defaults_to_scribe_v2_without_audio_events() {
        let fields = form_fields(&options("", "auto", &[]));
        assert_eq!(
            fields,
            vec![
                ("model_id", "scribe_v2".to_string()),
                ("tag_audio_events", "false".to_string()),
            ]
        );
    }

    #[test]
    fn sends_language_and_short_keyterms() {
        let fields = form_fields(&options(
            "scribe_v2",
            "fr",
            &["Samlu", "one two three four five six"],
        ));
        assert!(fields.contains(&("language_code", "fr".to_string())));
        let keyterms: Vec<_> = fields
            .iter()
            .filter(|(name, _)| *name == "keyterms")
            .map(|(_, value)| value.as_str())
            .collect();
        assert_eq!(keyterms, vec!["Samlu"]);
    }

    #[test]
    fn older_models_do_not_get_keyterms() {
        let fields = form_fields(&options("scribe_v1", "auto", &["Samlu"]));
        assert!(!fields.iter().any(|(name, _)| *name == "keyterms"));
    }
}
