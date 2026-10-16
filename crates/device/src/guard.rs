//! Making audio streams fail safe: whatever happens inside a stream (an error, a panic
//! in the processing callback) the stream is stopped and released, its OS handles are
//! closed and the engine is told, instead of a dead stream silently looking alive.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

/// Runs a closure when dropped, including while unwinding.
pub struct OnDrop<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> OnDrop<F> {
    pub fn new(f: F) -> Self {
        Self(Some(f))
    }
}

impl<F: FnOnce()> Drop for OnDrop<F> {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

/// The message of a caught panic payload.
pub fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

/// Runs `body` repeatedly until it returns `Ok(false)` (normal stop) or fails, then
/// calls `stop` exactly once. An error or a panic in `body` is reported through
/// `fault` (once) and ends the loop; a panic never escapes, so the owning thread can
/// release its COM objects and OS handles normally.
pub fn supervise_stream(
    stop: impl FnOnce(),
    mut body: impl FnMut() -> anyhow::Result<bool>,
    fault: &dyn Fn(String),
) {
    let _stop = OnDrop::new(stop);
    loop {
        match catch_unwind(AssertUnwindSafe(&mut body)) {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => break,
            Ok(Err(e)) => {
                fault(format!("{e:#}"));
                break;
            }
            Err(p) => {
                fault(format!(
                    "audio processing panicked: {}",
                    panic_message(p.as_ref())
                ));
                break;
            }
        }
    }
}

/// Wraps a real-time processing callback for a stream whose thread we do not own
/// (cpal). A panic in the callback is caught (it must not unwind into the driver's
/// thread), reported once through `fault` so the engine restarts, and the callback is
/// poisoned: from then on it outputs silence, since its state may be inconsistent.
pub struct GuardedCallback<F> {
    f: F,
    fault: Arc<dyn Fn(String) + Send + Sync>,
    poisoned: bool,
}

impl<F> GuardedCallback<F> {
    pub fn new(f: F, fault: Arc<dyn Fn(String) + Send + Sync>) -> Self {
        Self {
            f,
            fault,
            poisoned: false,
        }
    }

    /// Calls the wrapped callback with `buf`; on failure the buffer is filled with
    /// `silence` (the format's zero: 0 for signed/float, mid-scale for unsigned).
    pub fn call<T: Copy>(&mut self, buf: &mut [T], silence: T)
    where
        F: FnMut(&mut [T]),
    {
        if !self.poisoned {
            let f = &mut self.f;
            match catch_unwind(AssertUnwindSafe(|| f(buf))) {
                Ok(()) => return,
                Err(p) => {
                    self.poisoned = true;
                    (self.fault)(format!(
                        "audio processing panicked: {}",
                        panic_message(p.as_ref())
                    ));
                }
            }
        }
        buf.fill(silence);
    }

    /// Same for read-only (capture) buffers.
    pub fn call_ref<T>(&mut self, buf: &[T])
    where
        F: FnMut(&[T]),
    {
        if self.poisoned {
            return;
        }
        let f = &mut self.f;
        if let Err(p) = catch_unwind(AssertUnwindSafe(|| f(buf))) {
            self.poisoned = true;
            (self.fault)(format!(
                "audio processing panicked: {}",
                panic_message(p.as_ref())
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::{Arc, Mutex};

    type Seen = Arc<Mutex<Vec<String>>>;

    fn faults() -> (Seen, crate::wasapi_excl::Fault) {
        let v = Arc::new(Mutex::new(Vec::new()));
        let v2 = v.clone();
        (v, Arc::new(move |m| v2.lock().unwrap().push(m)))
    }

    #[test]
    fn a_panicking_stream_body_is_stopped_and_reported() {
        let stops = Cell::new(0);
        let (seen, fault) = faults();
        let mut n = 0;
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            supervise_stream(
                || stops.set(stops.get() + 1),
                || {
                    n += 1;
                    if n == 3 {
                        panic!("bug in the processing chain");
                    }
                    Ok(true)
                },
                &*fault,
            )
        }));
        assert!(r.is_ok(), "the panic must not escape the stream thread");
        assert_eq!(stops.get(), 1, "the stream must be stopped exactly once");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(seen[0].contains("bug in the processing chain"), "{seen:?}");
    }

    #[test]
    fn an_erroring_stream_is_stopped_and_reported() {
        let stops = Cell::new(0);
        let (seen, fault) = faults();
        supervise_stream(
            || stops.set(stops.get() + 1),
            || anyhow::bail!("AUDCLNT_E_DEVICE_INVALIDATED"),
            &*fault,
        );
        assert_eq!(stops.get(), 1);
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_normal_stop_reports_nothing() {
        let stops = Cell::new(0);
        let (seen, fault) = faults();
        let mut n = 0;
        supervise_stream(
            || stops.set(stops.get() + 1),
            || {
                n += 1;
                Ok(n < 5)
            },
            &*fault,
        );
        assert_eq!(stops.get(), 1);
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn a_panicking_callback_is_contained_and_silenced() {
        let (seen, fault) = faults();
        let mut calls = 0;
        let mut g = GuardedCallback::new(
            move |b: &mut [f32]| {
                calls += 1;
                b.fill(0.5);
                if calls == 2 {
                    panic!("index out of bounds");
                }
            },
            fault,
        );
        let mut out = Vec::new();
        for _ in 0..4 {
            let mut b = [0.0f32; 4];
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| g.call(&mut b, 0.0)));
            assert!(
                r.is_ok(),
                "the panic must not unwind into the audio driver thread"
            );
            out.push(b);
        }
        assert_eq!(out[0], [0.5; 4]);
        assert_eq!(
            out[1], [0.0; 4],
            "the buffer of the failed call is silenced"
        );
        assert_eq!(out[3], [0.0; 4], "a poisoned callback outputs silence");
        assert_eq!(seen.lock().unwrap().len(), 1, "reported exactly once");
    }

    #[test]
    fn unsigned_output_is_silenced_at_mid_scale() {
        let (_seen, fault) = faults();
        let mut g = GuardedCallback::new(|_: &mut [u16]| panic!("boom"), fault);
        let mut b = [7u16; 4];
        g.call(&mut b, 32_768);
        assert_eq!(b, [32_768; 4]);
    }

    #[test]
    fn a_panicking_capture_callback_is_contained() {
        let (seen, fault) = faults();
        let mut calls = 0;
        let mut g = GuardedCallback::new(
            move |_: &[f32]| {
                calls += 1;
                assert!(calls < 2, "chain state corrupted");
            },
            fault,
        );
        for _ in 0..3 {
            g.call_ref(&[0.1f32; 8]);
        }
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn on_drop_runs_while_unwinding() {
        let hit = Arc::new(Mutex::new(false));
        let h2 = hit.clone();
        let r = std::panic::catch_unwind(move || {
            let _g = OnDrop::new(move || *h2.lock().unwrap() = true);
            panic!("x");
        });
        assert!(r.is_err());
        assert!(*hit.lock().unwrap());
    }
}
