use base64::{Engine, engine::general_purpose::STANDARD};
use modular_agent_core::{
    Agent, AgentContext, AgentData, AgentError, AgentOutput, AgentSpec, AgentValue, AsAgent,
    ModularAgent, async_trait, modular_agent,
};
use regex::Regex;
use reqwest::Client;

const CATEGORY: &str = "TTS/VoiceVox";

const PORT_TEXT: &str = "text";
const PORT_AUDIO: &str = "audio";
const PORT_UNIT: &str = "unit";
const PORT_SPEAKERS: &str = "speakers";

const CONFIG_SPEAKER: &str = "speaker";
const CONFIG_SPEED: &str = "speed";
const CONFIG_PITCH: &str = "pitch";
const CONFIG_VOLUME: &str = "volume";
const CONFIG_URL: &str = "url";
const CONFIG_EMOTION_MAP: &str = "emotion_map";

const DEFAULT_URL: &str = "http://localhost:50021";

fn get_url(ma: &ModularAgent) -> String {
    ma.get_global_configs(VoiceVoxTtsAgent::DEF_NAME)
        .and_then(|cfg| cfg.get_string(CONFIG_URL).ok())
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| DEFAULT_URL.to_string())
}

// --- Emotion matching ---

struct EmotionMatcher {
    regex: Regex,
    keys: Vec<String>, // capture group index → emotion_map key
}

/// A text segment with an optional emotion tag key
struct EmotionSegment {
    key: Option<String>,
    text: String,
}

impl EmotionMatcher {
    /// Build from emotion_map keys. Returns None if no valid patterns.
    fn build(raw_keys: &[String]) -> Result<Option<Self>, AgentError> {
        let mut patterns: Vec<(String, String)> = Vec::new(); // (original_key, regex_pattern)

        for key in raw_keys {
            if key.is_empty() {
                continue;
            }
            let pattern = if key.starts_with('/') && key.ends_with('/') && key.len() >= 2 {
                // Raw regex: strip surrounding slashes
                key[1..key.len() - 1].to_string()
            } else {
                // Literal: escape for regex
                regex::escape(key)
            };
            if pattern.is_empty() {
                continue;
            }
            // Validate that the pattern doesn't match empty strings
            if let Ok(test_re) = Regex::new(&pattern)
                && test_re.is_match("")
            {
                continue; // Skip patterns that match empty strings
            }
            patterns.push((key.clone(), pattern));
        }

        if patterns.is_empty() {
            return Ok(None);
        }

        // Sort by key length descending (specific patterns before generic)
        patterns.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

        let mut keys = Vec::new();
        let mut regex_parts = Vec::new();
        for (key, pattern) in patterns {
            keys.push(key);
            regex_parts.push(format!("({})", pattern));
        }

        let combined = regex_parts.join("|");
        let regex = Regex::new(&combined).map_err(|e| {
            AgentError::InvalidConfig(format!("Invalid emotion_map pattern: {}", e))
        })?;

        Ok(Some(EmotionMatcher { regex, keys }))
    }

    /// Parse text into segments using the emotion patterns
    fn parse(&self, text: &str) -> Vec<EmotionSegment> {
        let mut segments = Vec::new();
        let mut last_end = 0;
        let mut current_key: Option<String> = None;

        for caps in self.regex.captures_iter(text) {
            let whole_match = caps.get(0).unwrap();
            let match_start = whole_match.start();
            let match_end = whole_match.end();

            // Text before this match belongs to the previous emotion (or None)
            if match_start > last_end {
                let segment_text = &text[last_end..match_start];
                if !segment_text.is_empty() {
                    segments.push(EmotionSegment {
                        key: current_key.clone(),
                        text: segment_text.to_string(),
                    });
                }
            }

            // Determine which capture group matched → which key
            let mut matched_key = None;
            for (i, key) in self.keys.iter().enumerate() {
                if caps.get(i + 1).is_some() {
                    matched_key = Some(key.clone());
                    break;
                }
            }
            current_key = matched_key;
            last_end = match_end;
        }

        // Remaining text after last match
        if last_end < text.len() {
            let remaining = &text[last_end..];
            if !remaining.is_empty() {
                segments.push(EmotionSegment {
                    key: current_key.clone(),
                    text: remaining.to_string(),
                });
            }
        }

        segments
    }
}

// --- WAV utilities ---

/// Find the offset and size of the "data" chunk in a WAV/RIFF file.
/// Returns (data_offset, data_size) where data_offset is the start of PCM data.
fn find_wav_data_chunk(wav: &[u8]) -> Result<(usize, u32), AgentError> {
    if wav.len() < 12 {
        return Err(AgentError::IoError("WAV data too short".into()));
    }
    // Verify RIFF header
    if &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return Err(AgentError::IoError("Invalid WAV header".into()));
    }
    // Scan for "data" chunk after the initial 12-byte RIFF header
    let mut pos = 12;
    while pos + 8 <= wav.len() {
        let chunk_id = &wav[pos..pos + 4];
        let chunk_size =
            u32::from_le_bytes([wav[pos + 4], wav[pos + 5], wav[pos + 6], wav[pos + 7]]);
        if chunk_id == b"data" {
            return Ok((pos + 8, chunk_size));
        }
        // Move to next chunk (chunk header is 8 bytes + chunk_size, padded to even)
        let advance = 8 + chunk_size as usize;
        let advance = if !advance.is_multiple_of(2) {
            advance + 1
        } else {
            advance
        };
        pos += advance;
    }
    Err(AgentError::IoError("WAV data chunk not found".into()))
}

/// Extract sample rate (4 bytes at offset 24) and number of channels (2 bytes at offset 22)
fn wav_format_info(wav: &[u8]) -> Result<(u32, u16), AgentError> {
    if wav.len() < 26 {
        return Err(AgentError::IoError(
            "WAV data too short for format info".into(),
        ));
    }
    let channels = u16::from_le_bytes([wav[22], wav[23]]);
    let sample_rate = u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]);
    Ok((sample_rate, channels))
}

/// Concatenate multiple WAV files into one.
/// All WAVs must have the same sample rate and channel count.
fn concatenate_wavs(wavs: &[Vec<u8>]) -> Result<Vec<u8>, AgentError> {
    if wavs.is_empty() {
        return Err(AgentError::IoError("No WAV data to concatenate".into()));
    }
    if wavs.len() == 1 {
        return Ok(wavs[0].clone());
    }

    let (ref_sample_rate, ref_channels) = wav_format_info(&wavs[0])?;

    // Collect PCM data from all WAVs and validate format consistency
    let mut pcm_chunks: Vec<&[u8]> = Vec::new();
    let mut total_pcm_size: usize = 0;

    for (i, wav) in wavs.iter().enumerate() {
        let (sample_rate, channels) = wav_format_info(wav)?;
        if sample_rate != ref_sample_rate || channels != ref_channels {
            return Err(AgentError::IoError(format!(
                "WAV format mismatch at segment {}: expected {}Hz {}ch, got {}Hz {}ch",
                i, ref_sample_rate, ref_channels, sample_rate, channels
            )));
        }
        let (data_offset, data_size) = find_wav_data_chunk(wav)?;
        let end = data_offset + data_size as usize;
        let end = end.min(wav.len());
        pcm_chunks.push(&wav[data_offset..end]);
        total_pcm_size += end - data_offset;
    }

    // Build output: use the first WAV's header, replace PCM data
    let (first_data_offset, _) = find_wav_data_chunk(&wavs[0])?;
    let header_size = first_data_offset; // everything before PCM data (includes "data" chunk header)

    // header_size includes up to (but not including) the PCM data
    // We need to include the chunk headers up to and including "data" + size field
    // first_data_offset already points past the "data" chunk's 8-byte header
    // So the header we want is wavs[0][0..first_data_offset]
    // But we need to rewrite the data chunk size and RIFF size

    let mut output = Vec::with_capacity(header_size + total_pcm_size);

    // Copy header from first WAV (up to PCM data start)
    output.extend_from_slice(&wavs[0][..header_size]);

    // Update the "data" chunk size (4 bytes before data_offset)
    let data_size_offset = header_size - 4;
    let total_pcm_u32 = total_pcm_size as u32;
    output[data_size_offset..data_size_offset + 4].copy_from_slice(&total_pcm_u32.to_le_bytes());

    // Append all PCM data
    for chunk in &pcm_chunks {
        output.extend_from_slice(chunk);
    }

    // Update RIFF chunk size (offset 4, 4 bytes): file_size - 8
    let riff_size = (output.len() - 8) as u32;
    output[4..8].copy_from_slice(&riff_size.to_le_bytes());

    Ok(output)
}

// --- Agent implementation ---

/// Synthesize speech from text using VoiceVox engine.
/// Supports emotion tags in text (e.g. `((happy))Hello`) when emotion_map is configured.
#[modular_agent(
    title = "VoiceVox TTS",
    category = CATEGORY,
    inputs = [PORT_TEXT],
    outputs = [PORT_AUDIO],
    integer_config(name = CONFIG_SPEAKER, default = 0, description = "Speaker ID (use VoiceVox Speakers agent to list available IDs)"),
    number_config(name = CONFIG_SPEED, default = 1.0, description = "Speech speed multiplier (1.0 = normal)"),
    number_config(name = CONFIG_PITCH, default = 0.0, description = "Pitch adjustment (0.0 = normal)"),
    number_config(name = CONFIG_VOLUME, default = 1.0, description = "Volume multiplier (1.0 = normal)"),
    object_config(name = CONFIG_EMOTION_MAP, title = "Emotion Map", description = "Pattern to parameter overrides. Keys are literal strings matched in text. Wrap in / for regex. e.g. {\"((happy))\": {\"speaker\": 1, \"pitch\": 0.1}}"),
    custom_global_config(name = CONFIG_URL, type_ = "string", default = AgentValue::string(DEFAULT_URL), title = "VoiceVox URL"),
    hint(width = 1, height = 2),
)]
struct VoiceVoxTtsAgent {
    data: AgentData,
    client: Client,
    cached_emotion_matcher: Option<EmotionMatcher>,
    emotion_cache_built: bool,
}

impl VoiceVoxTtsAgent {
    /// Synthesize a single text segment into WAV bytes
    async fn synthesize_segment(
        &self,
        url: &str,
        text: &str,
        speaker: i64,
        speed: f64,
        pitch: f64,
        volume: f64,
    ) -> Result<Vec<u8>, AgentError> {
        let speaker_str = speaker.to_string();

        // Step 1: Create audio query
        let audio_query_url = format!("{}/audio_query", url);
        let resp = self
            .client
            .post(&audio_query_url)
            .query(&[("text", text), ("speaker", &speaker_str)])
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() {
                    AgentError::IoError(format!("VoiceVox engine is not running at {}: {}", url, e))
                } else {
                    AgentError::IoError(format!("audio_query request error: {}", e))
                }
            })?;

        let resp = resp
            .error_for_status()
            .map_err(|e| AgentError::IoError(format!("audio_query failed: {}", e)))?;

        let mut audio_query: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AgentError::IoError(format!("audio_query response parse error: {}", e)))?;

        // Adjust synthesis parameters
        audio_query["speedScale"] = serde_json::json!(speed);
        audio_query["pitchScale"] = serde_json::json!(pitch);
        audio_query["volumeScale"] = serde_json::json!(volume);

        // Step 2: Synthesize audio
        let synthesis_url = format!("{}/synthesis", url);
        let resp = self
            .client
            .post(&synthesis_url)
            .query(&[("speaker", &speaker_str)])
            .json(&audio_query)
            .send()
            .await
            .map_err(|e| AgentError::IoError(format!("synthesis request error: {}", e)))?;

        let resp = resp
            .error_for_status()
            .map_err(|e| AgentError::IoError(format!("synthesis failed: {}", e)))?;

        let wav_bytes = resp
            .bytes()
            .await
            .map_err(|e| AgentError::IoError(format!("synthesis response read error: {}", e)))?;

        Ok(wav_bytes.to_vec())
    }

    /// Build or return cached EmotionMatcher from current emotion_map config
    fn get_emotion_matcher(&mut self) -> Result<Option<&EmotionMatcher>, AgentError> {
        if !self.emotion_cache_built {
            let emotion_map = self.configs()?.get_object_or_default(CONFIG_EMOTION_MAP);
            if !emotion_map.is_empty() {
                let keys: Vec<String> = emotion_map.keys().cloned().collect();
                self.cached_emotion_matcher = EmotionMatcher::build(&keys)?;
            } else {
                self.cached_emotion_matcher = None;
            }
            self.emotion_cache_built = true;
        }
        Ok(self.cached_emotion_matcher.as_ref())
    }

    /// Extract override parameters for a given emotion key from the emotion_map config
    fn get_emotion_overrides(
        &self,
        key: &str,
        default_speaker: i64,
        default_speed: f64,
        default_pitch: f64,
        default_volume: f64,
    ) -> Result<(i64, f64, f64, f64), AgentError> {
        let emotion_map = self.configs()?.get_object_or_default(CONFIG_EMOTION_MAP);
        if let Some(overrides) = emotion_map.get(key)
            && let Some(obj) = overrides.as_object()
        {
            let speaker = obj
                .get(CONFIG_SPEAKER)
                .and_then(|v| v.as_i64())
                .unwrap_or(default_speaker);
            let speed = obj
                .get(CONFIG_SPEED)
                .and_then(|v| v.as_f64())
                .unwrap_or(default_speed);
            let pitch = obj
                .get(CONFIG_PITCH)
                .and_then(|v| v.as_f64())
                .unwrap_or(default_pitch);
            let volume = obj
                .get(CONFIG_VOLUME)
                .and_then(|v| v.as_f64())
                .unwrap_or(default_volume);
            return Ok((speaker, speed, pitch, volume));
        }
        Ok((
            default_speaker,
            default_speed,
            default_pitch,
            default_volume,
        ))
    }
}

#[async_trait]
impl AsAgent for VoiceVoxTtsAgent {
    fn new(ma: ModularAgent, id: String, spec: AgentSpec) -> Result<Self, AgentError> {
        Ok(Self {
            data: AgentData::new(ma, id, spec),
            client: Client::new(),
            cached_emotion_matcher: None,
            emotion_cache_built: false,
        })
    }

    fn configs_changed(&mut self) -> Result<(), AgentError> {
        self.cached_emotion_matcher = None;
        self.emotion_cache_built = false;
        Ok(())
    }

    async fn process(
        &mut self,
        ctx: AgentContext,
        _port: String,
        value: AgentValue,
    ) -> Result<(), AgentError> {
        let text = value
            .as_str()
            .map(str::to_string)
            .or_else(|| value.as_message().map(|m| m.content.clone()))
            .or_else(|| value.get_str("text").map(str::to_string))
            .ok_or_else(|| {
                AgentError::InvalidValue("Input must be a string, message, or doc".into())
            })?;

        if text.is_empty() {
            return Err(AgentError::InvalidValue("Input text is empty".to_string()));
        }

        let url = get_url(self.ma());
        let config = self.configs()?;
        let default_speaker = config.get_integer_or(CONFIG_SPEAKER, 0);
        let default_speed = config.get_number_or(CONFIG_SPEED, 1.0);
        let default_pitch = config.get_number_or(CONFIG_PITCH, 0.0);
        let default_volume = config.get_number_or(CONFIG_VOLUME, 1.0);

        // Check if emotion parsing is configured
        let has_emotion = self.get_emotion_matcher()?.is_some();

        if !has_emotion {
            // No emotion_map: synthesize the entire text as-is (original behavior)
            let wav = self
                .synthesize_segment(
                    &url,
                    &text,
                    default_speaker,
                    default_speed,
                    default_pitch,
                    default_volume,
                )
                .await?;
            let b64 = STANDARD.encode(&wav);
            let data_uri = format!("data:audio/wav;base64,{}", b64);
            return self
                .output(ctx, PORT_AUDIO, AgentValue::string(data_uri))
                .await;
        }

        // Parse text into emotion segments
        let matcher = self.cached_emotion_matcher.as_ref().unwrap();
        let segments = matcher.parse(&text);

        // Filter out empty text segments
        let segments: Vec<&EmotionSegment> = segments
            .iter()
            .filter(|s| !s.text.trim().is_empty())
            .collect();

        if segments.is_empty() {
            return Err(AgentError::InvalidValue(
                "No text content after emotion tag parsing".to_string(),
            ));
        }

        // Synthesize each segment
        let mut wav_parts: Vec<Vec<u8>> = Vec::with_capacity(segments.len());
        for segment in &segments {
            let (speaker, speed, pitch, volume) = if let Some(key) = &segment.key {
                self.get_emotion_overrides(
                    key,
                    default_speaker,
                    default_speed,
                    default_pitch,
                    default_volume,
                )?
            } else {
                (
                    default_speaker,
                    default_speed,
                    default_pitch,
                    default_volume,
                )
            };

            let wav = self
                .synthesize_segment(&url, &segment.text, speaker, speed, pitch, volume)
                .await?;
            wav_parts.push(wav);
        }

        // Concatenate WAV parts
        let combined = concatenate_wavs(&wav_parts)?;
        let b64 = STANDARD.encode(&combined);
        let data_uri = format!("data:audio/wav;base64,{}", b64);

        self.output(ctx, PORT_AUDIO, AgentValue::string(data_uri))
            .await
    }
}

/// List available speakers and styles from VoiceVox engine
#[modular_agent(
    title = "VoiceVox Speakers",
    category = CATEGORY,
    inputs = [PORT_UNIT],
    outputs = [PORT_SPEAKERS],
)]
struct VoiceVoxSpeakersAgent {
    data: AgentData,
    client: Client,
}

#[async_trait]
impl AsAgent for VoiceVoxSpeakersAgent {
    fn new(ma: ModularAgent, id: String, spec: AgentSpec) -> Result<Self, AgentError> {
        Ok(Self {
            data: AgentData::new(ma, id, spec),
            client: Client::new(),
        })
    }

    async fn process(
        &mut self,
        ctx: AgentContext,
        _port: String,
        _value: AgentValue,
    ) -> Result<(), AgentError> {
        let url = get_url(self.ma());
        let speakers_url = format!("{}/speakers", url);

        let resp = self.client.get(&speakers_url).send().await.map_err(|e| {
            if e.is_connect() {
                AgentError::IoError(format!("VoiceVox engine is not running at {}: {}", url, e))
            } else {
                AgentError::IoError(format!("speakers request error: {}", e))
            }
        })?;

        let resp = resp
            .error_for_status()
            .map_err(|e| AgentError::IoError(format!("speakers request failed: {}", e)))?;

        let speakers: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AgentError::IoError(format!("speakers response parse error: {}", e)))?;

        let speakers = AgentValue::from_serialize(&speakers)?;
        self.output(ctx, PORT_SPEAKERS, speakers).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- EmotionMatcher tests ---

    #[test]
    fn test_build_empty_keys() {
        let result = EmotionMatcher::build(&[]).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_build_with_empty_string_keys() {
        let result = EmotionMatcher::build(&["".to_string()]).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_build_literal_keys() {
        let keys = vec!["((happy))".to_string(), "((sad))".to_string()];
        let matcher = EmotionMatcher::build(&keys).unwrap();
        assert!(matcher.is_some());
    }

    #[test]
    fn test_build_regex_keys() {
        let keys = vec!["/\\(\\(\\w+\\)\\)/".to_string()];
        let matcher = EmotionMatcher::build(&keys).unwrap();
        assert!(matcher.is_some());
    }

    #[test]
    fn test_build_invalid_regex() {
        let keys = vec!["/[invalid/".to_string()];
        let result = EmotionMatcher::build(&keys);
        assert!(result.is_err());
    }

    #[test]
    fn test_build_skips_empty_match_patterns() {
        // Pattern that matches empty string should be skipped
        let keys = vec!["/.*/".to_string()];
        let result = EmotionMatcher::build(&keys).unwrap();
        assert!(result.is_none());
    }

    // --- Parsing tests ---

    #[test]
    fn test_parse_no_tags() {
        let keys = vec!["((happy))".to_string()];
        let matcher = EmotionMatcher::build(&keys).unwrap().unwrap();
        let segments = matcher.parse("Hello world");
        assert_eq!(segments.len(), 1);
        assert!(segments[0].key.is_none());
        assert_eq!(segments[0].text, "Hello world");
    }

    #[test]
    fn test_parse_single_tag() {
        let keys = vec!["((happy))".to_string()];
        let matcher = EmotionMatcher::build(&keys).unwrap().unwrap();
        let segments = matcher.parse("((happy))Hello world");
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].key.as_deref(), Some("((happy))"));
        assert_eq!(segments[0].text, "Hello world");
    }

    #[test]
    fn test_parse_multiple_tags() {
        let keys = vec!["((neutral))".to_string(), "((happy))".to_string()];
        let matcher = EmotionMatcher::build(&keys).unwrap().unwrap();
        let segments = matcher.parse("((neutral))こんにちは。((happy))元気だった？");
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].key.as_deref(), Some("((neutral))"));
        assert_eq!(segments[0].text, "こんにちは。");
        assert_eq!(segments[1].key.as_deref(), Some("((happy))"));
        assert_eq!(segments[1].text, "元気だった？");
    }

    #[test]
    fn test_parse_text_before_first_tag() {
        let keys = vec!["((happy))".to_string()];
        let matcher = EmotionMatcher::build(&keys).unwrap().unwrap();
        let segments = matcher.parse("Hello ((happy))World");
        assert_eq!(segments.len(), 2);
        assert!(segments[0].key.is_none());
        assert_eq!(segments[0].text, "Hello ");
        assert_eq!(segments[1].key.as_deref(), Some("((happy))"));
        assert_eq!(segments[1].text, "World");
    }

    #[test]
    fn test_parse_consecutive_tags() {
        let keys = vec!["((angry))".to_string(), "((happy))".to_string()];
        let matcher = EmotionMatcher::build(&keys).unwrap().unwrap();
        let segments = matcher.parse("((angry))((happy))Text");
        // ((angry)) followed immediately by ((happy)) — no text for angry
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].key.as_deref(), Some("((happy))"));
        assert_eq!(segments[0].text, "Text");
    }

    #[test]
    fn test_parse_tag_at_end() {
        let keys = vec!["((sad))".to_string()];
        let matcher = EmotionMatcher::build(&keys).unwrap().unwrap();
        let segments = matcher.parse("Hello((sad))");
        assert_eq!(segments.len(), 1);
        assert!(segments[0].key.is_none());
        assert_eq!(segments[0].text, "Hello");
    }

    #[test]
    fn test_parse_generic_regex_pattern() {
        let keys = vec!["/\\(\\(\\w+\\)\\)/".to_string()];
        let matcher = EmotionMatcher::build(&keys).unwrap().unwrap();
        let segments = matcher.parse("((neutral))こんにちは。((excited))すごい！");
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].key.as_deref(), Some("/\\(\\(\\w+\\)\\)/"));
        assert_eq!(segments[0].text, "こんにちは。");
        assert_eq!(segments[1].text, "すごい！");
    }

    #[test]
    fn test_parse_complex_example() {
        let keys = vec![
            "((neutral))".to_string(),
            "((happy))".to_string(),
            "((sad))".to_string(),
            "((angry))".to_string(),
            "((relaxed))".to_string(),
        ];
        let matcher = EmotionMatcher::build(&keys).unwrap().unwrap();
        let segments =
            matcher.parse("((neutral))夏休みの予定か～。((happy))海に遊びに行こうかな！");
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].key.as_deref(), Some("((neutral))"));
        assert_eq!(segments[0].text, "夏休みの予定か～。");
        assert_eq!(segments[1].key.as_deref(), Some("((happy))"));
        assert_eq!(segments[1].text, "海に遊びに行こうかな！");
    }

    // --- WAV utility tests ---

    fn make_test_wav(pcm_data: &[u8]) -> Vec<u8> {
        let data_size = pcm_data.len() as u32;
        let riff_size = 36 + data_size; // 36 = size of headers after RIFF chunk id+size
        let mut wav = Vec::new();
        // RIFF header
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&riff_size.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        // fmt chunk
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // chunk size
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM format
        wav.extend_from_slice(&1u16.to_le_bytes()); // 1 channel
        wav.extend_from_slice(&24000u32.to_le_bytes()); // sample rate
        wav.extend_from_slice(&48000u32.to_le_bytes()); // byte rate
        wav.extend_from_slice(&2u16.to_le_bytes()); // block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        // data chunk
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_size.to_le_bytes());
        wav.extend_from_slice(pcm_data);
        wav
    }

    fn make_test_wav_with_extra_chunk(pcm_data: &[u8]) -> Vec<u8> {
        let data_size = pcm_data.len() as u32;
        // Add a LIST chunk before data to test non-44-byte offset
        let list_data = b"INFO";
        let list_size = list_data.len() as u32;
        let riff_size = 36 + 8 + list_size + data_size;
        let mut wav = Vec::new();
        // RIFF header
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&riff_size.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        // fmt chunk
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // 1 channel
        wav.extend_from_slice(&24000u32.to_le_bytes());
        wav.extend_from_slice(&48000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        // LIST chunk (extra, before data)
        wav.extend_from_slice(b"LIST");
        wav.extend_from_slice(&list_size.to_le_bytes());
        wav.extend_from_slice(list_data);
        // data chunk
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_size.to_le_bytes());
        wav.extend_from_slice(pcm_data);
        wav
    }

    #[test]
    fn test_find_wav_data_chunk_standard() {
        let wav = make_test_wav(&[1, 2, 3, 4]);
        let (offset, size) = find_wav_data_chunk(&wav).unwrap();
        assert_eq!(size, 4);
        assert_eq!(&wav[offset..offset + 4], &[1, 2, 3, 4]);
    }

    #[test]
    fn test_find_wav_data_chunk_with_extra() {
        let wav = make_test_wav_with_extra_chunk(&[5, 6, 7, 8]);
        let (offset, size) = find_wav_data_chunk(&wav).unwrap();
        assert_eq!(size, 4);
        assert_eq!(&wav[offset..offset + 4], &[5, 6, 7, 8]);
        // Verify offset is NOT 44
        assert_ne!(offset, 44);
    }

    #[test]
    fn test_find_wav_data_chunk_invalid() {
        let result = find_wav_data_chunk(&[0; 10]);
        assert!(result.is_err());
    }

    #[test]
    fn test_wav_format_info() {
        let wav = make_test_wav(&[]);
        let (sample_rate, channels) = wav_format_info(&wav).unwrap();
        assert_eq!(sample_rate, 24000);
        assert_eq!(channels, 1);
    }

    #[test]
    fn test_concatenate_single_wav() {
        let wav = make_test_wav(&[1, 2, 3, 4]);
        let result = concatenate_wavs(&[wav.clone()]).unwrap();
        assert_eq!(result, wav);
    }

    #[test]
    fn test_concatenate_two_wavs() {
        let wav1 = make_test_wav(&[1, 2, 3, 4]);
        let wav2 = make_test_wav(&[5, 6, 7, 8]);
        let result = concatenate_wavs(&[wav1, wav2]).unwrap();

        // Verify it's a valid WAV
        assert_eq!(&result[0..4], b"RIFF");
        assert_eq!(&result[8..12], b"WAVE");

        // Find data chunk and verify combined PCM
        let (offset, size) = find_wav_data_chunk(&result).unwrap();
        assert_eq!(size, 8);
        assert_eq!(&result[offset..offset + 8], &[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn test_concatenate_format_mismatch() {
        let wav1 = make_test_wav(&[1, 2]);
        // Create a WAV with different sample rate
        let mut wav2 = make_test_wav(&[3, 4]);
        // Overwrite sample rate at offset 24
        wav2[24..28].copy_from_slice(&44100u32.to_le_bytes());
        let result = concatenate_wavs(&[wav1, wav2]);
        assert!(result.is_err());
    }

    #[test]
    fn test_concatenate_empty() {
        let result = concatenate_wavs(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_concatenate_with_extra_chunks() {
        let wav1 = make_test_wav_with_extra_chunk(&[1, 2]);
        let wav2 = make_test_wav_with_extra_chunk(&[3, 4]);
        let result = concatenate_wavs(&[wav1, wav2]).unwrap();

        let (offset, size) = find_wav_data_chunk(&result).unwrap();
        assert_eq!(size, 4);
        assert_eq!(&result[offset..offset + 4], &[1, 2, 3, 4]);
    }
}
