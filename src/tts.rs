use base64::{Engine, engine::general_purpose::STANDARD};
use modular_agent_core::{
    Agent, AgentContext, AgentData, AgentError, AgentOutput, AgentSpec, AgentValue, AsAgent,
    ModularAgent, async_trait, modular_agent,
};
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

const DEFAULT_URL: &str = "http://localhost:50021";

fn get_url(ma: &ModularAgent) -> String {
    ma.get_global_configs(VoiceVoxTtsAgent::DEF_NAME)
        .and_then(|cfg| cfg.get_string(CONFIG_URL).ok())
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| DEFAULT_URL.to_string())
}

/// Synthesize speech from text using VoiceVox engine
#[modular_agent(
    title = "VoiceVox TTS",
    category = CATEGORY,
    inputs = [PORT_TEXT],
    outputs = [PORT_AUDIO],
    integer_config(name = CONFIG_SPEAKER, default = 0, description = "Speaker ID (use VoiceVox Speakers agent to list available IDs)"),
    number_config(name = CONFIG_SPEED, default = 1.0, description = "Speech speed multiplier (1.0 = normal)"),
    number_config(name = CONFIG_PITCH, default = 0.0, description = "Pitch adjustment (0.0 = normal)"),
    number_config(name = CONFIG_VOLUME, default = 1.0, description = "Volume multiplier (1.0 = normal)"),
    custom_global_config(name = CONFIG_URL, type_ = "string", default = AgentValue::string(DEFAULT_URL), title = "VoiceVox URL"),
    hint(width = 1, height = 2),
)]
struct VoiceVoxTtsAgent {
    data: AgentData,
    client: Client,
}

#[async_trait]
impl AsAgent for VoiceVoxTtsAgent {
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
        let speaker = config.get_integer_or(CONFIG_SPEAKER, 0).to_string();
        let speed_scale = config.get_number_or(CONFIG_SPEED, 1.0);
        let pitch_scale = config.get_number_or(CONFIG_PITCH, 0.0);
        let volume_scale = config.get_number_or(CONFIG_VOLUME, 1.0);

        // Step 1: Create audio query
        let audio_query_url = format!("{}/audio_query", url);
        let resp = self
            .client
            .post(&audio_query_url)
            .query(&[("text", text.as_str()), ("speaker", speaker.as_str())])
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
        audio_query["speedScale"] = serde_json::json!(speed_scale);
        audio_query["pitchScale"] = serde_json::json!(pitch_scale);
        audio_query["volumeScale"] = serde_json::json!(volume_scale);

        // Step 2: Synthesize audio
        let synthesis_url = format!("{}/synthesis", url);
        let resp = self
            .client
            .post(&synthesis_url)
            .query(&[("speaker", &speaker)])
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

        // Encode as data URI
        let b64 = STANDARD.encode(&wav_bytes);
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
