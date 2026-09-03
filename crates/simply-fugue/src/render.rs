//! The `render_audio` tool contract: what an agent sends, and what comes back.
//!
//! Both hosts take the same argument under the same tool name and return the
//! same shape, so an agent driving `render_audio(fugue)` on one and then the
//! other does no argument conversion at either hop. The two hosts nonetheless
//! answer *different questions* — nexus renders a fugue through a hosted
//! plugin, flux through one patch — so identical calls give comparable, not
//! equal, audio. A tool description built on these types should say so.
//!
//! Moved here from `nexus-tools` so parity on the return leg is **type
//! identity** rather than two structs somebody keeps in step by hand.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::composite::CompositeFugue;

/// The instance every request targets unless it says otherwise.
pub fn default_instance() -> String {
    "default".into()
}

fn is_default_instance(s: &str) -> bool {
    s == "default"
}

/// Render a fugue offline, without an audio driver.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RenderAudioRequest {
    #[serde(default = "default_instance", skip_serializing_if = "is_default_instance")]
    pub instance: String,
    /// Musical fugue performance to render.
    pub fugue: CompositeFugue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tempo_bpm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_size: Option<usize>,
    /// "wav_base64" (default) or "file".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_format: Option<String>,
    /// Target file path when output_format is "file".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
}

/// What a render produced, and what it measured.
///
/// `Default` is derived so a host can fill the fields it has and leave the rest
/// with `..Default::default()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
pub struct RenderAudioResponse {
    pub sample_rate: u32,
    pub channels: usize,
    pub frames: usize,
    pub duration_s: f64,
    pub peak_db: f64,
    pub rms_db: f64,
    pub is_silent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wav_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,

    // --- added by the superset; all three cost nothing on the wire when unused.
    /// What the host decided on the caller's behalf — a CC that reached
    /// nothing, channels collapsed onto one instrument, an event kind dropped.
    /// Silence is the failure mode this exists to prevent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// Linear RMS, alongside `rms_db`. dB loses silence to `-inf`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rms: Option<f32>,
    /// Linear peak amplitude, alongside `peak_db`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_amplitude: Option<f32>,
}
