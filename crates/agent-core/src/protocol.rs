use crate::config::Exposure;
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u8 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Identity {
    pub id: String,
    pub name: String,
    pub os: String,
    pub version: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Metrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_percent: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_percent: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_muted: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub protocol: u8,
    pub device: Identity,
    pub exposure: Exposure,
    pub metrics: Metrics,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientMessage {
    Action {
        id: String,
        action: String,
        expires_at: u64,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Snapshot {
        data: Snapshot,
    },
    ActionResult {
        id: String,
        success: bool,
        error: Option<String>,
    },
    Error {
        error: String,
    },
}
