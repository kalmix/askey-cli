use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Deserialize, Serialize)]
pub struct AskeyFile {
    pub v: String,
    pub n: Option<String>,
    pub m: Metadata,
    pub p: HashMap<String, String>,
    pub d: Option<u64>,
    pub fr: Frames,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Metadata {
    pub f: usize,
    pub d: u64,
    pub t: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Frames {
    Simple(Vec<String>),
    Detailed(Vec<FrameObj>),
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FrameObj {
    pub c: String,
    pub d: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuantizationLevel {
    Standard,
    Toon,
    Posterized,
    Retro,
}

impl QuantizationLevel {
    pub fn to_u8(self) -> u8 {
        match self {
            Self::Standard => 16,
            Self::Toon => 32,
            Self::Posterized => 48,
            Self::Retro => 64,
        }
    }

    pub fn from_u8(val: u8) -> Self {
        match val {
            32 => Self::Toon,
            48 => Self::Posterized,
            64 => Self::Retro,
            _ => Self::Standard,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Standard => "16 (standard)",
            Self::Toon => "32 (toon)",
            Self::Posterized => "48 (posterized)",
            Self::Retro => "64 (retro)",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Standard => Self::Toon,
            Self::Toon => Self::Posterized,
            Self::Posterized => Self::Retro,
            Self::Retro => Self::Retro,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Standard => Self::Standard,
            Self::Toon => Self::Standard,
            Self::Posterized => Self::Toon,
            Self::Retro => Self::Posterized,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CharsetPreset {
    Blocks,
    Standard,
    Detailed,
    Custom(String),
}

impl CharsetPreset {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Blocks => "blocks",
            Self::Standard => "standard",
            Self::Detailed => "detailed",
            Self::Custom(s) => s.as_str(),
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "blocks" => Self::Blocks,
            "standard" => Self::Standard,
            "detailed" => Self::Detailed,
            other => Self::Custom(other.to_string()),
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Blocks => Self::Standard,
            Self::Standard => Self::Detailed,
            Self::Detailed => Self::Blocks,
            Self::Custom(_) => Self::Blocks,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Blocks => Self::Detailed,
            Self::Standard => Self::Blocks,
            Self::Detailed => Self::Standard,
            Self::Custom(_) => Self::Detailed,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub default_noclip: bool,
    #[serde(default)]
    pub default_dashboard: bool,
}


