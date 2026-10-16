//! Lock-free primitives shared by the audio callbacks and the UI.
//!
//! Under `--cfg loom` the std atomics are swapped for loom's, so the model checker can
//! explore every interleaving of the code below and of [`crate::ring::Ring`]:
//!
//! ```text
//! cargo test --release -p core-dsp --lib --target x86_64-pc-windows-msvc \
//!     --config "target.x86_64-pc-windows-msvc.rustflags=['--cfg','loom']" -- loom
//! ```

#[cfg(loom)]
pub(crate) use loom::sync::atomic::{AtomicU32, AtomicUsize};
use std::sync::atomic::Ordering::Relaxed;
#[cfg(not(loom))]
pub(crate) use std::sync::atomic::{AtomicU32, AtomicUsize};

/// An `f32` in an `AtomicU32` (bit pattern), for meters and single values.
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
    /// Peak-hold update for positive values, whose bit patterns order like the numbers.
    /// Zero is a no-op; -0.0 must be rejected explicitly: it compares `>= 0.0` but its
    /// bit pattern (0x8000_0000) beats every positive float and would hide later peaks.
    pub fn fetch_max(&self, v: f32) {
        if v.is_finite() && v > 0.0 {
            self.0.fetch_max(v.to_bits(), Relaxed);
        }
    }
    /// Read and reset to zero.
    pub fn take(&self) -> f32 {
        f32::from_bits(self.0.swap(0, Relaxed))
    }
}

#[cfg(all(test, not(loom)))]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn negative_zero_does_not_hide_later_peaks() {
        let m = AtomicF32::new(0.0);
        m.fetch_max(-0.0);
        m.fetch_max(0.5);
        assert_eq!(m.take(), 0.5);
    }

    #[test]
    fn negative_and_non_finite_values_are_ignored() {
        let m = AtomicF32::new(0.0);
        for v in [-1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            m.fetch_max(v);
        }
        assert_eq!(m.take(), 0.0);
    }

    /// Several audio threads publish peaks while the UI drains the meter: no peak may
    /// be lost, so the largest value seen across all takes is the largest value written.
    #[test]
    fn concurrent_peaks_are_never_lost() {
        let m = Arc::new(AtomicF32::new(0.0));
        let writers: Vec<_> = (0..4)
            .map(|w| {
                let m = m.clone();
                std::thread::spawn(move || {
                    for i in 0..200_000u32 {
                        let v = ((i * 7 + w * 13) % 1000) as f32 / 1000.0;
                        m.fetch_max(if i % 97 == 0 { -0.0 } else { v });
                    }
                    m.fetch_max(1.0 + w as f32);
                })
            })
            .collect();
        let mut seen = 0.0f32;
        while writers.iter().any(|t| !t.is_finished()) {
            seen = seen.max(m.take());
        }
        for t in writers {
            t.join().unwrap();
        }
        seen = seen.max(m.take());
        assert_eq!(seen, 4.0);
    }
}

#[cfg(all(test, loom))]
mod loom_tests {
    use super::*;
    use loom::sync::Arc;
    use loom::thread;

    #[test]
    fn loom_meter_peak_survives_concurrent_take() {
        loom::model(|| {
            let m = Arc::new(AtomicF32::new(0.0));
            let a = {
                let m = m.clone();
                thread::spawn(move || m.fetch_max(-0.0))
            };
            let b = {
                let m = m.clone();
                thread::spawn(move || m.fetch_max(0.75))
            };
            let first = m.take();
            a.join().unwrap();
            b.join().unwrap();
            let second = m.take();
            // The 0.75 peak is reported by exactly one of the two takes.
            assert_eq!(first.max(second), 0.75, "first {first}, second {second}");
        });
    }
}
