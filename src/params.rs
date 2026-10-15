//! All user-tweakable processing parameters. Plain `Copy` data so the audio
//! thread can take a snapshot without allocating.

use serde::{Deserialize, Serialize};

/// Internal processing rate. RNNoise requires 48 kHz.
pub const SAMPLE_RATE: f32 = 48_000.0;
/// Processing block size (10 ms at 48 kHz, RNNoise's native frame).
pub const FRAME: usize = 480;
pub const EQ_BANDS: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelMode {
    Mix,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InputParams {
    pub channel: ChannelMode,
    pub gain_db: f32,
    pub phase_invert: bool,
    pub lowcut_on: bool,
    pub lowcut_hz: f32,
}
impl Default for InputParams {
    fn default() -> Self {
        Self {
            channel: ChannelMode::Mix,
            gain_db: 0.0,
            phase_invert: false,
            lowcut_on: true,
            lowcut_hz: 80.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DenoiseParams {
    pub enabled: bool,
    /// 0 = fully dry, 1 = fully RNNoise output.
    pub strength: f32,
}
impl Default for DenoiseParams {
    fn default() -> Self {
        Self {
            enabled: true,
            strength: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GateParams {
    pub enabled: bool,
    pub threshold_db: f32,
    pub hysteresis_db: f32,
    pub attack_ms: f32,
    pub hold_ms: f32,
    pub release_ms: f32,
    /// Attenuation applied while closed (negative dB).
    pub range_db: f32,
}
impl Default for GateParams {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_db: -50.0,
            hysteresis_db: 4.0,
            attack_ms: 2.0,
            hold_ms: 80.0,
            release_ms: 150.0,
            range_db: -60.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BandKind {
    LowShelf,
    Peak,
    HighShelf,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct EqBand {
    pub enabled: bool,
    pub kind: BandKind,
    pub freq: f32,
    pub gain_db: f32,
    pub q: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EqParams {
    pub enabled: bool,
    pub bands: [EqBand; EQ_BANDS],
}
impl Default for EqParams {
    fn default() -> Self {
        let b = |kind, freq, gain_db, q| EqBand {
            enabled: true,
            kind,
            freq,
            gain_db,
            q,
        };
        Self {
            enabled: true,
            bands: [
                b(BandKind::LowShelf, 100.0, 0.0, 0.7),
                b(BandKind::Peak, 250.0, 0.0, 1.0),
                b(BandKind::Peak, 1000.0, 0.0, 1.0),
                b(BandKind::Peak, 4000.0, 0.0, 1.0),
                b(BandKind::HighShelf, 9000.0, 0.0, 0.7),
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeEssParams {
    pub enabled: bool,
    /// Centre of the sibilance band.
    pub freq_hz: f32,
    /// Band level above which reduction starts.
    pub threshold_db: f32,
    pub ratio: f32,
    /// Largest reduction applied (positive dB).
    pub max_reduction_db: f32,
}
impl Default for DeEssParams {
    fn default() -> Self {
        Self {
            enabled: true,
            freq_hz: 6500.0,
            threshold_db: -32.0,
            ratio: 4.0,
            max_reduction_db: 10.0,
        }
    }
}

/// Gate driven by RNNoise's voice-activity probability instead of level.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VadGateParams {
    pub enabled: bool,
    /// Probability (0..1) above which the gate opens.
    pub threshold: f32,
    pub hold_ms: f32,
    pub release_ms: f32,
    /// Attenuation while closed (negative dB).
    pub range_db: f32,
}
impl Default for VadGateParams {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold: 0.5,
            hold_ms: 150.0,
            release_ms: 120.0,
            range_db: -30.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompParams {
    pub enabled: bool,
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub knee_db: f32,
    pub makeup_db: f32,
    pub auto_makeup: bool,
}
impl Default for CompParams {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_db: -20.0,
            ratio: 3.0,
            attack_ms: 8.0,
            release_ms: 120.0,
            knee_db: 6.0,
            makeup_db: 3.0,
            auto_makeup: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LimiterParams {
    pub enabled: bool,
    pub ceiling_db: f32,
    pub release_ms: f32,
}
impl Default for LimiterParams {
    fn default() -> Self {
        Self {
            enabled: true,
            ceiling_db: -1.0,
            release_ms: 60.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputParams {
    pub gain_db: f32,
    pub mute: bool,
}
impl Default for OutputParams {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            mute: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChainParams {
    /// Skip the whole chain (input gain still applies).
    pub bypass: bool,
    pub input: InputParams,
    pub denoise: DenoiseParams,
    pub gate: GateParams,
    pub vad_gate: VadGateParams,
    pub eq: EqParams,
    pub deess: DeEssParams,
    pub comp: CompParams,
    pub limiter: LimiterParams,
    pub output: OutputParams,
}
impl ChainParams {
    /// Everything off except the safety limiter.
    pub fn flat() -> Self {
        let mut p = Self::default();
        p.input.lowcut_on = false;
        p.denoise.enabled = false;
        p.gate.enabled = false;
        p.vad_gate.enabled = false;
        p.eq.enabled = false;
        p.deess.enabled = false;
        p.comp.enabled = false;
        p
    }

    /// Warm, present voice for Discord / calls.
    pub fn voice() -> Self {
        let mut p = Self::default();
        p.eq.bands[0].gain_db = -2.0; // tame proximity boom
        p.eq.bands[1].gain_db = -2.5; // clear mud
        p.eq.bands[3].gain_db = 2.5; // presence
        p.eq.bands[4].gain_db = 1.5; // air
        p.comp.threshold_db = -22.0;
        p.comp.ratio = 3.5;
        p
    }

    /// Heavier gating/compression for noisy rooms and games.
    pub fn noisy_room() -> Self {
        let mut p = Self::voice();
        p.gate.threshold_db = -42.0;
        p.gate.hold_ms = 120.0;
        p.input.lowcut_hz = 110.0;
        p.comp.ratio = 4.0;
        p.comp.threshold_db = -24.0;
        p
    }

    pub fn builtin() -> Vec<(&'static str, ChainParams)> {
        vec![
            ("Voice (Discord / calls)", Self::voice()),
            ("Noisy room", Self::noisy_room()),
            ("Flat (limiter only)", Self::flat()),
        ]
    }
}
