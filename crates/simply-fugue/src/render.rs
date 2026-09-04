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

/// Where a render's audio comes back.
///
/// A closed set, and that is the change: `output_format` used to be a bare
/// `Option<String>` that only ever tested for `"file"`, so **any** other
/// spelling — `"wav"`, `"File"`, a typo — fell through to base64 and returned
/// tens of megabytes of inline audio nobody asked for. An unknown value is now
/// a deserialize error that names both accepted spellings (FUGUE-API.md §3.5,
/// ruled in §6.6).
///
/// The two spellings that already worked parse unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    /// Write a WAV file and return its path. **The default** — the loop is
    /// `render_audio` → `analyze(wav)` in another process, which reads a file
    /// (FUGUE-API.md §6.6).
    #[default]
    File,
    /// Return the WAV inline, base64-encoded.
    WavBase64,
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
    /// `"file"` (the default) or `"wav_base64"`. Absent means
    /// [`OutputFormat::File`]; [`RenderAudioRequest::output_format`] resolves
    /// it, so no host repeats the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_format: Option<OutputFormat>,
    /// Target file path when output_format is "file".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
}

impl RenderAudioRequest {
    /// The format this request asks for, with the absent case resolved.
    ///
    /// `Option` is kept on the field so a host can still tell "the caller said
    /// `file`" from "the caller said nothing"; every host that only wants to
    /// know *what to write* calls this instead of spelling the default again.
    pub fn output_format(&self) -> OutputFormat {
        self.output_format.unwrap_or_default()
    }
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
