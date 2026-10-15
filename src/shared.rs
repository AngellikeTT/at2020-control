//! State shared between the UI thread and the real-time audio callbacks.

use crate::params::ChainParams;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::Mutex;

pub struct AtomicF32(AtomicU32);
impl AtomicF32 {
    pub fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }
    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Relaxed))
    }
    pub fn store(&self, v: f32) {
        self.0.store(v.to_bits(), Relaxed)
    }
    /// Peak-hold update; only valid for non-negative values (bit order == numeric order).
    pub fn fetch_max(&self, v: f32) {
        if v.is_finite() && v >= 0.0 {
            self.0.fetch_max(v.to_bits(), Relaxed);
        }
    }
    /// Read and reset to zero.
    pub fn take(&self) -> f32 {
        f32::from_bits(self.0.swap(0, Relaxed))
    }
}

/// Parameter hand-off: UI writes under the mutex and bumps `version`; the audio
/// thread notices the bump and `try_lock`s (never blocks).
pub struct ParamsShared {
    pub version: AtomicU64,
    pub data: Mutex<ChainParams>,
}
impl ParamsShared {
    pub fn new(p: ChainParams) -> Self {
        Self {
            version: AtomicU64::new(1),
            data: Mutex::new(p),
        }
    }
    pub fn set(&self, p: ChainParams) {
        if let Ok(mut g) = self.data.lock() {
            *g = p;
        }
        self.version.fetch_add(1, Relaxed);
    }
}

pub struct Meters {
    pub in_peak: AtomicF32,
    pub out_peak: AtomicF32,
    pub gr_db: AtomicF32,
    pub vad: AtomicF32,
    pub gate_open: AtomicBool,
}

pub struct Shared {
    pub params: ParamsShared,
    pub meters: Meters,
    pub monitor_vol: AtomicF32,
    pub monitor_active: AtomicBool,
    pub out_fill: AtomicUsize,
    pub underruns: AtomicU64,
    pub fault_flag: AtomicBool,
    pub fault: Mutex<Option<String>>,
}

impl Shared {
    pub fn new(p: ChainParams) -> Self {
        Self {
            params: ParamsShared::new(p),
            meters: Meters {
                in_peak: AtomicF32::new(0.0),
                out_peak: AtomicF32::new(0.0),
                gr_db: AtomicF32::new(0.0),
                vad: AtomicF32::new(0.0),
                gate_open: AtomicBool::new(true),
            },
            monitor_vol: AtomicF32::new(0.7),
            monitor_active: AtomicBool::new(false),
            out_fill: AtomicUsize::new(0),
            underruns: AtomicU64::new(0),
            fault_flag: AtomicBool::new(false),
            fault: Mutex::new(None),
        }
    }
    pub fn set_fault(&self, msg: String) {
        if let Ok(mut f) = self.fault.lock() {
            *f = Some(msg);
        }
        self.fault_flag.store(true, Relaxed);
    }
    pub fn take_fault(&self) -> Option<String> {
        if self.fault_flag.swap(false, Relaxed) {
            self.fault.lock().ok().and_then(|mut f| f.take())
        } else {
            None
        }
    }
}
