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
   modular-agent-voicevox = { path = "../../modular-agent-voicevox" }
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

## VoiceVox Speakers

### Configuration

No configuration required. Uses the shared global VoiceVox URL config from VoiceVox TTS.

### Ports

- **Input**: `unit` — Any value (triggers speaker list fetch)
- **Output**: `speakers` — JSON array of available speakers and their styles

## Architecture

Both agents share the VoiceVox Engine URL via a `custom_global_config` on the VoiceVox TTS agent, accessed through a `get_url()` helper. Each agent holds its own `reqwest::Client` for HTTP connection pooling.

## License

Apache-2.0 OR MIT
