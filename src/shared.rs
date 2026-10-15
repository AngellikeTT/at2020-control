//! State shared between the UI thread and the real-time audio callbacks.
//! Everything the audio thread touches is lock-free: atomics, SPSC rings and a
//! triple buffer for the parameter set.

use crate::params::ChainParams;
use crate::ring::Ring;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use triple_buffer::{triple_buffer, Input, Output};

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

/// Parameter hand-off. The UI publishes into a triple buffer; the audio thread reads
/// the newest published value without blocking, locking or allocating.
pub struct ParamsShared {
    last: Mutex<ChainParams>,
    writer: Mutex<Option<Input<ChainParams>>>,
}

impl ParamsShared {
    pub fn new(p: ChainParams) -> Self {
        Self {
            last: Mutex::new(p),
            writer: Mutex::new(None),
        }
    }

    /// Called by the UI thread.
    pub fn set(&self, p: ChainParams) {
        if let Ok(mut l) = self.last.lock() {
            *l = p;
        }
        if let Ok(mut w) = self.writer.lock() {
            if let Some(w) = w.as_mut() {
                w.write(p);
            }
        }
    }

    pub fn current(&self) -> ChainParams {
        self.last.lock().map(|l| *l).unwrap_or_default()
    }

    /// Creates a fresh reader seeded with the latest parameters. Each engine start
    /// attaches once; a previous reader simply stops receiving updates.
    pub fn attach(&self) -> Output<ChainParams> {
        let p = self.current();
        let (input, output) = triple_buffer(&p);
        if let Ok(mut w) = self.writer.lock() {
            *w = Some(input);
        }
        output
    }
}

pub struct Meters {
    pub in_peak: AtomicF32,
    pub out_peak: AtomicF32,
    pub gr_db: AtomicF32,
    pub deess_gr_db: AtomicF32,
    pub vad: AtomicF32,
    pub gate_open: AtomicBool,
    pub vad_gate_open: AtomicBool,
}

/// Which signal the recorder taps.
pub const REC_PROCESSED: u32 = 0;
pub const REC_RAW: u32 = 1;

pub struct Shared {
    pub params: ParamsShared,
    pub meters: Meters,
    pub monitor_vol: AtomicF32,
    pub monitor_active: AtomicBool,
    pub out_fill: AtomicUsize,
    pub underruns: AtomicU64,
    pub fault_flag: AtomicBool,
    pub fault: Mutex<Option<String>>,
    /// Post-chain mono samples for the spectrum analyzer.
    pub spec_out: Arc<Ring>,
    /// Pre-chain (after input gain/mixdown) mono samples for the analyzer.
    pub spec_in: Arc<Ring>,
    /// Recorder tap; written only while `rec_active`.
    pub rec_ring: Arc<Ring>,
    pub rec_active: AtomicBool,
    pub rec_source: AtomicU32,
    /// Number of audio threads registered with MMCSS "Pro Audio".
    pub mmcss_threads: AtomicU32,
}

impl Shared {
    pub fn new(p: ChainParams) -> Self {
        Self {
            params: ParamsShared::new(p),
            meters: Meters {
                in_peak: AtomicF32::new(0.0),
                out_peak: AtomicF32::new(0.0),
                gr_db: AtomicF32::new(0.0),
                deess_gr_db: AtomicF32::new(0.0),
                vad: AtomicF32::new(0.0),
                gate_open: AtomicBool::new(true),
                vad_gate_open: AtomicBool::new(true),
            },
            monitor_vol: AtomicF32::new(0.7),
            monitor_active: AtomicBool::new(false),
            out_fill: AtomicUsize::new(0),
            underruns: AtomicU64::new(0),
            fault_flag: AtomicBool::new(false),
            fault: Mutex::new(None),
            spec_out: Arc::new(Ring::new(1 << 14)),
            spec_in: Arc::new(Ring::new(1 << 14)),
            rec_ring: Arc::new(Ring::new(1 << 18)),
            rec_active: AtomicBool::new(false),
            rec_source: AtomicU32::new(REC_PROCESSED),
            mmcss_threads: AtomicU32::new(0),
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
