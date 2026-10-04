# Samlu Voice module — design

Date: 2026-10-04
Status: Approved in brainstorming, pending written-spec review

## Context

Samlu launches as one macOS app with two modules users can enable
independently: **Voice** (a Wispr Flow–style speech-to-text companion) and
**Launcher** (a Raycast-class Spotlight alternative). Agent notifications are
removed from this launch and become a separate utility later. The audience is
power users and prosumers: keyboard-heavy Mac users across roles, addressed
with mainstream copy but dense, configurable UI.

This spec covers the **Voice module only**. The shared shell (module toggles,
removing agent hooks, settings layout) and the Launcher module each get their
own spec.

### Starting point

Voice already ships: tap/hold `Option+V`, Summary (`+Shift`) and Prompt
(`+Command`) modes, editable preview, retry/recovery that never loses a
capture, an OpenAI-compatible cloud path (Groq default), a separate optional
transformation endpoint, and per-endpoint keys in macOS Keychain. Normal
dictation pastes the raw transcript; only Summary and Prompt call an LLM.

## Goals

1. Transcribe on-device with **whisper.cpp** (Metal) and **Apple
   SpeechAnalyzer** (macOS 26+).
2. Let users use whisper models they already have on disk, add any model file,
   or download a curated one.
3. Support cloud STT through **OpenAI-compatible presets** (Groq, OpenAI,
   custom URL), **Deepgram**, and **ElevenLabs Scribe**.
4. Optional **AI cleanup** of normal dictation.
5. **Custom vocabulary**, **language selection**, and local **transcript
   history**.
6. Existing users keep working configurations with no action.

## Non-goals

- Parakeet, sherpa-onnx, MLX, or any other local engine.
- AssemblyAI or other non-listed cloud STT providers.
- Streaming / live partial transcripts.
- Resumable downloads.
- Re-pasting from history; searching voice history from the launcher.
- Module enable/disable toggles and agent-hook removal (shell spec).
- Changing Summary or Prompt mode behavior.

## Decisions

| Topic | Decision |
|---|---|
| Packaging | One app; Voice is one of two modules |
| Engine integration | In-process engine layer (approach A); no sidecars |
| Local engines | whisper.cpp via `whisper-rs` (Metal); Apple SpeechAnalyzer via Swift static lib |
| Cloud STT | OpenAI-compatible presets, Deepgram, ElevenLabs Scribe |
| Cleanup | Optional, off by default; on failure paste raw transcript |
| Prompt mode | Kept as-is |
| History | On by default, local only, 200 entries, no audio |

## Architecture

### Engine layer — `voice/engines/`

One entry point:

```rust
pub async fn transcribe(audio: &PreparedAudio, opts: &SttOptions) -> Result<String, ProviderError>
```

`SttOptions` carries the selected engine, model, base URL, language, and
vocabulary. `PreparedAudio` holds the original WAV bytes (for cloud engines)
and, lazily, 16 kHz mono `f32` samples (for local engines).

| Module | Transport | Notes |
|---|---|---|
| `openai_compat` | Existing `cloud::transcribe`, moved here | Presets: Groq, OpenAI, Custom URL |
| `deepgram` | `POST /v1/listen`, raw WAV body | `Authorization: Token <key>`, model `nova-3`, `keyterm` per vocabulary term |
| `elevenlabs` | `POST /v1/speech-to-text`, multipart | `xi-api-key` header, model `scribe_v1` |
| `whisper_cpp` | `whisper-rs`, in-process, Metal | Run inside `tokio::task::spawn_blocking` |
| `apple` | Swift static library, `@_cdecl` C entry point | Offered only when the runtime OS is macOS 26+ |

All engines return `ProviderError`. Local engines return only permanent
errors, so `cloud::with_retries` never retries them. The existing retry,
recovery, preview, and delivery code is unchanged.

`produce_text` in `voice/mod.rs` replaces its direct `cloud::transcribe` call
with `engines::transcribe`. The cached `Capture.transcript` behavior is kept,
so a retry after a failed transform does not re-transcribe.

### Audio preparation

The recorder already produces mono WAV at the device rate. A new resampler
converts to 16 kHz mono `f32` for local engines. It runs once per capture and
its output is cached on the capture so retries reuse it. Cloud engines keep
receiving the WAV unchanged.

### Model library — `voice/models.rs`

Tracks whisper.cpp models from three sources in one list:

1. **Downloaded** — a built-in catalog of official `ggerganov/whisper.cpp`
   GGML models: `base`, `small`, `medium`, `large-v3-turbo`,
   `large-v3-turbo-q5_0`. Each entry has URL, byte size, SHA-256, and a short
   label. Files are stored in `<app data>/models/`. Downloads stream to
   `<name>.part`, report progress to settings, verify SHA-256, then rename.
   Cancel deletes the `.part`. Interrupted downloads restart.
2. **Discovered** — scanned on opening Voice settings and on "Rescan".
   Locations: MacWhisper's model folder, Superwhisper's model folder, and the
   Hugging Face hub cache entry for `ggerganov/whisper.cpp`. Discovered files
   are referenced in place, never copied.
3. **Added** — "Add model file…" opens a file picker for any `.bin`.

Every candidate is validated by reading the GGML header magic and model
hyperparameters. Invalid files are rejected with "Not a whisper.cpp model".
A referenced file that disappears is listed as **Missing**. Samlu-downloaded
models can be deleted from disk; discovered or added models can only be
removed from the list.

**Loading.** The selected model loads on hotkey press (in parallel with
recording) and stays resident. It is unloaded after 10 minutes without use, or
immediately when the user selects a different model.

**Apple engine assets.** SpeechAnalyzer is listed as "Apple (on-device)" with a
language picker. If the selected locale's system assets are not installed,
Samlu requests installation through the system asset API and shows its
progress.

### Settings — `voice/config.rs`

The flat `provider`, `base_url`, `stt_model` fields are replaced by:

```json
"stt": {
  "engine": "openai_compat | deepgram | elevenlabs | whisper_cpp | apple",
  "preset": "groq | openai | custom | null",
  "model": "whisper-large-v3-turbo",
  "base_url": "https://api.groq.com/openai/v1",
  "language": "auto"
},
"cleanup_dictation": false,
"vocabulary": [],
"keep_history": true,
"models": [ { "path": "...", "source": "downloaded | discovered | added" } ]
```

**Migration.** On load, a config without `stt` is converted to
`engine: "openai_compat"` with the existing provider, base URL, and model;
`preset` is `groq` when the provider was `groq`, else `custom`. Old fields are
then dropped on the next save.

Keychain service names for existing OpenAI-compatible endpoints are unchanged,
so stored keys keep working. Deepgram and ElevenLabs use
`com.samlu.desktop.voice.deepgram` and `com.samlu.desktop.voice.elevenlabs`.

The transformation endpoint stays OpenAI-compatible and separate. New presets:
Ollama (`http://localhost:11434/v1`) and LM Studio
(`http://localhost:1234/v1`); these do not require an API key.

### Cleanup, vocabulary, language

**Cleanup.** When `cleanup_dictation` is on, Normal mode calls `transform`
with a cleanup system prompt: remove fillers, fix punctuation and casing,
apply spoken self-corrections, preserve wording, and never answer or act on
the content. The transcript is wrapped in explicit delimiters in the user
message. If cleanup fails after retries, Samlu delivers the raw transcript and
shows "Cleanup skipped" in the capsule. Summary and Prompt keep their current
failure behavior (recovery state).

**Vocabulary.** A list of terms in `voice-config.json`, edited in Voice
settings. Applied per engine:

| Engine | Mechanism |
|---|---|
| whisper.cpp | `initial_prompt`, truncated to fit Whisper's prompt window |
| OpenAI-compatible | `prompt` form field |
| Deepgram | One `keyterm` query parameter per term |
| ElevenLabs | Provider key-term parameter if Scribe supports it; otherwise not sent |
| Apple | Analyzer contextual strings if exposed; otherwise not sent |
| Cleanup / Summary / Prompt LLM | Appended to the system prompt as terms to spell exactly |

**Language.** `"auto"` or an ISO 639-1 code, mapped to each engine's language
parameter (Deepgram uses `detect_language=true` for auto). Models whose file
name ends in `.en.bin` force `en`, and the picker shows this. Apple has no
auto-detection; `auto` resolves to the system locale.

### Transcript history — `voice/history.rs`

Modeled on `launcher/clipboard.rs`: a `VecDeque` persisted to
`voice-history.json`, capacity 200, oldest dropped first.

Entry fields: `id`, `timestamp`, `mode`, `text` (delivered), `raw` (present
only when different from `text`), `engine_label`, `target_app`, `outcome`
(`inserted | copied | preview_cancelled | cleanup_skipped`).

Audio is never stored. Captures that fail before producing text are not
recorded; they remain in the existing in-memory recovery state.

`keep_history` defaults to `true`. Turning it off clears the in-memory list and
deletes the file. "Clear history" does the same without changing the toggle.

UI: a History section in Voice settings with case-insensitive substring
search, a per-entry copy button, and a raw/final toggle. The README "Security
and data" section gains a line describing local transcript history.

## Error handling

| Failure | Behavior |
|---|---|
| Selected model missing or invalid | Permanent error "Model not found — choose another in Voice settings"; capture kept for retry |
| Apple locale unsupported or assets not installed | Permanent error with an action to open Voice settings |
| Download network failure or SHA mismatch | `.part` deleted; error shown on the model row; nothing added to the list |
| Missing API key | Existing "No API key saved for this endpoint." |
| Cleanup transform fails | Raw transcript delivered; "Cleanup skipped" shown; history outcome `cleanup_skipped` |

## Onboarding

The voice step begins with **On this Mac (private, no account)** or **Cloud
provider**.

- On this Mac, macOS 26+: Apple engine, no download.
- On this Mac, older macOS: offer `large-v3-turbo-q5_0` (~550 MB) on Apple
  Silicon or `small` on Intel, and list any discovered models.
- Cloud provider: provider picker (Groq, OpenAI, Deepgram, ElevenLabs, Custom)
  and key entry.

Users with an existing config skip this step via migration.

## Build and CI

- `build.rs` compiles the Swift bridge into a static library alongside the
  existing Objective-C sources. Swift code uses `if #available(macOS 26, *)`;
  the deployment target stays macOS 11.
- `whisper-rs` builds whisper.cpp with CMake and the Metal backend for both
  `aarch64` and `x86_64`.
- CI and release workflows move from `macos-14` to a runner image that
  provides Xcode 26 (required for the SpeechAnalyzer SDK).
- The universal build and the ad-hoc signing path must be verified with the
  new native code.

## Testing

**Unit tests (`cargo test`, no network):**

- Config migration from the current flat JSON, including Groq and custom
  providers, and Keychain service names unchanged.
- Resampler: output length and dominant frequency for 48 kHz and 44.1 kHz
  input.
- GGML header validation on a valid header, a truncated file, and a non-GGML
  file.
- Vocabulary truncation for the whisper prompt window.
- Deepgram and ElevenLabs request construction: URL, headers, query and form
  parameters, without sending.
- Cleanup prompt wraps the transcript in delimiters.
- History capacity eviction; `keep_history = false` clears memory and deletes
  the file.
- `.en.bin` models force language `en`.

**Ignored integration tests (`cargo test -- --ignored`):**

- whisper.cpp transcribes a short checked-in WAV with `tiny.en`, downloaded
  during test setup (model not committed).
- Apple engine smoke test, skipped unless running on macOS 26+.

**Manual QA:** each engine end-to-end into TextEdit with cleanup on and off;
an interrupted and a checksum-failing download; a deleted referenced model;
upgrade from an existing Groq config.

## Items to confirm during planning

These have defined fallback behavior above; planning confirms the exact API or
path before implementation.

1. On-disk model folders for MacWhisper and Superwhisper.
2. Whether ElevenLabs Scribe accepts key terms, and the parameter name.
3. The SpeechAnalyzer contextual-strings API and the asset-installation API.
4. SHA-256 values and sizes for the catalog models.
5. The GitHub Actions runner image that provides Xcode 26.
