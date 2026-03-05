# VoiceVox TTS Agents for Modular Agent

Text-to-speech agents using [VoiceVox Engine](https://github.com/VOICEVOX/voicevox_engine). Requires VoiceVox Engine running locally.

[English](README.md) | [日本語](README_ja.md)

## Features

- **VoiceVox TTS** — Synthesize speech from text using VoiceVox engine
- **VoiceVox Speakers** — List available speakers and styles from VoiceVox engine

## Installation

Two changes to add this package to [`modular-agent-desktop`](https://github.com/modular-agent/modular-agent-desktop):

1. **`modular-agent-desktop/src-tauri/Cargo.toml`** — add dependency:

   ```toml
   modular-agent-voicevox = { git = "https://github.com/modular-agent/modular-agent-voicevox", tag = "v0.2.0" }
   ```

2. **`modular-agent-desktop/src-tauri/src/lib.rs`** — add import:

   ```rust
   #[allow(unused_imports)]
   use modular_agent_voicevox;
   ```

## Feature Flags

| Feature | Default | Description |
| ------- | ------- | ----------- |
| `default-tls` | Yes | Use platform-native TLS (via reqwest) |
| `rustls-tls` | No | Use rustls TLS instead of platform-native |

## VoiceVox TTS

### Configuration

| Config | Type | Default | Description |
| ------ | ---- | ------- | ----------- |
| speaker | integer | 0 | Speaker ID (use VoiceVox Speakers agent to list available IDs) |
| speed | number | 1.0 | Speech speed multiplier (1.0 = normal) |
| pitch | number | 0.0 | Pitch adjustment (0.0 = normal) |
| volume | number | 1.0 | Volume multiplier (1.0 = normal) |
| emotion_map | object | {} | Pattern to parameter overrides for emotion tags (see [Emotion Tags](#emotion-tags)) |

### Global Configuration

| Config | Type | Default | Description |
| ------ | ---- | ------- | ----------- |
| url | string | `http://localhost:50021` | VoiceVox Engine URL (shared across all VoiceVox agents). For [AivisSpeech](https://aivis-project.com/#products-aivisspeech), use `http://localhost:10101` (adjust to match your AivisSpeech settings) |

### Ports

- **Input**: `text` — Text string, message, or document with a `text` field
- **Output**: `audio` — WAV audio as a base64 data URI (`data:audio/wav;base64,...`)

### Output Format

WAV binary encoded as a data URI string: `data:audio/wav;base64,...`

Compatible with the Audio Player agent for playback.

### API Flow

Two-step process against the VoiceVox Engine:

1. `POST /audio_query?text=...&speaker=...` → AudioQuery JSON
2. `POST /synthesis?speaker=...` (body: AudioQuery JSON) → WAV binary

Speed, pitch, and volume configs are applied to the AudioQuery between steps 1 and 2.

### Emotion Tags

Input text may contain emotion tags like `((happy))Hello`. When `emotion_map` is configured, the agent parses these tags, splits text into segments, synthesizes each with per-emotion parameters, and concatenates the WAV output into a single data URI.

#### emotion_map format

Keys are literal strings matched in input text. Values are objects that override `speaker`, `speed`, `pitch`, and/or `volume` for that emotion. Unspecified parameters fall back to the agent's default config.

```json
{
  "((happy))": { "speaker": 1, "speed": 1.2, "pitch": 0.1 },
  "((sad))": { "speaker": 2, "pitch": -0.2 },
  "((angry))": { "speaker": 3, "speed": 1.1, "volume": 1.3 }
}
```

Keys wrapped in `/` are treated as raw regex patterns. For example, to strip all `((...))` tags with default parameters:

```json
{
  "/\\(\\(\\w+\\)\\)/": {}
}
```

When `emotion_map` is empty (default), emotion parsing is disabled and the agent behaves as a standard TTS.

## VoiceVox Speakers

### Configuration

No configuration required. Uses the shared global VoiceVox URL config from VoiceVox TTS.

### Ports

- **Input**: `unit` — Any value (triggers speaker list fetch)
- **Output**: `speakers` — JSON array of available speakers and their styles

## Architecture

Both agents share the VoiceVox Engine URL via a `custom_global_config` on the VoiceVox TTS agent, accessed through a `get_url()` helper. Each agent holds its own `reqwest::Client` for HTTP connection pooling. When emotion tags produce multiple segments, each is synthesized independently and the resulting WAV files are concatenated by parsing RIFF headers and merging PCM data.

## Key Dependencies

- [reqwest](https://crates.io/crates/reqwest) — HTTP client for VoiceVox Engine API
- [base64](https://crates.io/crates/base64) — WAV binary to data URI encoding
- [regex](https://crates.io/crates/regex) — Emotion tag pattern matching

## License

Apache-2.0 OR MIT
