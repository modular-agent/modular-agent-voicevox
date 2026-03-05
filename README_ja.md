# VoiceVox TTS Agents for Modular Agent

[VoiceVox Engine](https://github.com/VOICEVOX/voicevox_engine) を使った音声合成エージェント。VoiceVox Engine がローカルで起動している必要があります。

[English](README.md) | [日本語](README_ja.md)

## 機能

- **VoiceVox TTS** — VoiceVox エンジンを使ってテキストから音声を合成
- **VoiceVox Speakers** — VoiceVox エンジンで利用可能な話者とスタイルを一覧取得

## インストール

[`modular-agent-desktop`](https://github.com/modular-agent/modular-agent-desktop) にこのパッケージを追加するには、2箇所の変更が必要です:

1. **`modular-agent-desktop/src-tauri/Cargo.toml`** — 依存関係を追加:

   ```toml
   modular-agent-voicevox = { path = "../../modular-agent-voicevox" }
   ```

2. **`modular-agent-desktop/src-tauri/src/lib.rs`** — インポートを追加:

   ```rust
   #[allow(unused_imports)]
   use modular_agent_voicevox;
   ```

## Feature Flags

| Feature | デフォルト | 説明 |
| ------- | ---------- | ---- |
| `default-tls` | Yes | プラットフォームネイティブの TLS を使用 (reqwest 経由) |
| `rustls-tls` | No | ネイティブ TLS の代わりに rustls を使用 |

## VoiceVox TTS

### 設定

| Config | 型 | デフォルト | 説明 |
| ------ | -- | ---------- | ---- |
| speaker | integer | 0 | 話者 ID (VoiceVox Speakers エージェントで利用可能な ID を確認) |
| speed | number | 1.0 | 話速の倍率 (1.0 = 通常) |
| pitch | number | 0.0 | ピッチ調整 (0.0 = 通常) |
| volume | number | 1.0 | 音量の倍率 (1.0 = 通常) |

### グローバル設定

| Config | 型 | デフォルト | 説明 |
| ------ | -- | ---------- | ---- |
| url | string | `http://localhost:50021` | VoiceVox Engine の URL (全 VoiceVox エージェントで共有)。[AivisSpeech](https://aivis-project.com/#products-aivisspeech) を使う場合は `http://localhost:10101` に変更 (AivisSpeech の設定に合わせてください) |

### ポート

- **入力**: `text` — テキスト文字列、メッセージ、または `text` フィールドを持つドキュメント
- **出力**: `audio` — base64 データ URI 形式の WAV 音声 (`data:audio/wav;base64,...`)

### 出力形式

WAV バイナリを data URI 文字列としてエンコード: `data:audio/wav;base64,...`

Audio Player エージェントと組み合わせて再生できます。

### API フロー

VoiceVox Engine に対する2ステップのプロセス:

1. `POST /audio_query?text=...&speaker=...` → AudioQuery JSON
2. `POST /synthesis?speaker=...` (ボディ: AudioQuery JSON) → WAV バイナリ

speed、pitch、volume の設定はステップ 1 と 2 の間で AudioQuery に適用されます。

## VoiceVox Speakers

### 設定

設定不要。VoiceVox TTS のグローバル URL 設定を共有します。

### ポート

- **入力**: `unit` — 任意の値 (話者一覧の取得をトリガー)
- **出力**: `speakers` — 利用可能な話者とスタイルの JSON 配列

## アーキテクチャ

両エージェントは VoiceVox TTS エージェントの `custom_global_config` を通じて VoiceVox Engine の URL を共有し、`get_url()` ヘルパーでアクセスします。各エージェントは HTTP コネクションプーリングのために独自の `reqwest::Client` を保持しています。

## ライセンス

Apache-2.0 OR MIT
