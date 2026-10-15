//! Test recording: a writer thread drains the audio thread's tap ring into a WAV file.

use crate::params::SAMPLE_RATE;
use crate::shared::Shared;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

pub struct Recorder {
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    frames: Arc<AtomicU64>,
    join: Option<JoinHandle<Result<()>>>,
    pub path: PathBuf,
}

impl Recorder {
    /// `source`: [`crate::shared::REC_PROCESSED`] or [`crate::shared::REC_RAW`].
    pub fn start(shared: Arc<Shared>, path: &Path, source: u32) -> Result<Self> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).ok();
        }
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: SAMPLE_RATE as u32,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut w = hound::WavWriter::create(path, spec)
            .with_context(|| format!("creating {}", path.display()))?;
        // Drop anything left over from an earlier recording.
        let mut sink = vec![0.0f32; 4096];
        while shared.rec_ring.pop(&mut sink) > 0 {}
        shared.rec_source.store(source, Relaxed);
        shared.rec_active.store(true, Relaxed);

        let stop = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(AtomicU64::new(0));
        let (s2, f2, st2) = (shared.clone(), frames.clone(), stop.clone());
        let join = std::thread::Builder::new()
            .name("recorder".into())
            .spawn(move || -> Result<()> {
                let mut buf = vec![0.0f32; 4096];
                loop {
                    let n = s2.rec_ring.pop(&mut buf);
                    for v in &buf[..n] {
                        w.write_sample(*v)?;
                    }
                    f2.fetch_add(n as u64, Relaxed);
                    if n == 0 {
                        if st2.load(Relaxed) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
                w.finalize()?;
                Ok(())
            })
            .context("starting recorder thread")?;
        Ok(Self {
            shared,
            stop,
            frames,
            join: Some(join),
            path: path.to_path_buf(),
        })
    }

    pub fn seconds(&self) -> f32 {
        self.frames.load(Relaxed) as f32 / SAMPLE_RATE
    }

    /// Stops recording and finalises the file. Returns the recorded length in seconds.
    pub fn finish(mut self) -> Result<f32> {
        self.shared.rec_active.store(false, Relaxed);
        self.stop.store(true, Relaxed);
        if let Some(j) = self.join.take() {
            j.join()
                .map_err(|_| anyhow::anyhow!("recorder thread panicked"))??;
        }
        Ok(self.seconds())
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.shared.rec_active.store(false, Relaxed);
        self.stop.store(true, Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}
