//! Classifies captured audio so a dead input is reported as such instead of being
//! measured as a very quiet one. A USB microphone that is muted on the device (for
//! example the AT2020USB-X's touch mute) delivers *exact* zeros: no analogue noise
//! floor survives, so "every sample is 0.0" is the signature to look for.

use crate::dsp::lin_to_db;

/// Below this RMS a microphone signal is reported as suspiciously low (no real
/// capsule plus preamp is this quiet; even a muted analogue path leaves more noise).
pub const VERY_LOW_RMS_DB: f32 = -100.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SignalClass {
    /// Nothing was captured at all.
    Empty,
    /// Samples were captured but every one is exactly zero.
    DigitalSilence,
    /// Nonzero, but far below any real microphone's self-noise.
    VeryLow { rms_db: f32, peak_db: f32 },
    /// A real signal (noise floor or more).
    Present { rms_db: f32, peak_db: f32 },
}

impl SignalClass {
    pub fn is_signal(&self) -> bool {
        matches!(self, SignalClass::Present { .. })
    }

    pub fn describe(&self) -> String {
        match self {
            SignalClass::Empty => "no samples captured".into(),
            SignalClass::DigitalSilence => {
                "exact digital silence (every sample is 0.0): the device itself is sending \
                 zeros, e.g. a hardware/touch mute on the microphone"
                    .into()
            }
            SignalClass::VeryLow { rms_db, peak_db } => {
                format!("suspiciously low: RMS {rms_db:.1} dBFS, peak {peak_db:.1} dBFS")
            }
            SignalClass::Present { rms_db, peak_db } => {
                format!("RMS {rms_db:.1} dBFS, peak {peak_db:.1} dBFS")
            }
        }
    }
}

pub fn classify(x: &[f32]) -> SignalClass {
    if x.is_empty() {
        return SignalClass::Empty;
    }
    let (mut peak, mut sum) = (0.0f32, 0.0f64);
    for &v in x {
        // NaN/inf are not silence; count them as full scale.
        let a = if v.is_finite() { v.abs() } else { 1.0 };
        peak = peak.max(a);
        sum += (a as f64) * (a as f64);
    }
    if peak == 0.0 {
        return SignalClass::DigitalSilence;
    }
    let rms_db = lin_to_db((sum / x.len() as f64).sqrt() as f32);
    let peak_db = lin_to_db(peak);
    if rms_db < VERY_LOW_RMS_DB {
        SignalClass::VeryLow { rms_db, peak_db }
    } else {
        SignalClass::Present { rms_db, peak_db }
    }
}

/// For a peak meter (linear, non-negative): did any nonzero sample pass?
pub fn peak_is_signal(peak: f32) -> bool {
    peak.is_finite() && peak > 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_zeros_are_digital_silence() {
        assert_eq!(classify(&[0.0; 4800]), SignalClass::DigitalSilence);
        assert_eq!(classify(&[-0.0; 16]), SignalClass::DigitalSilence);
        assert!(!classify(&[0.0; 16]).is_signal());
    }

    #[test]
    fn empty_is_not_signal() {
        assert_eq!(classify(&[]), SignalClass::Empty);
    }

    #[test]
    fn a_single_lsb_is_not_silence_but_is_very_low() {
        let mut x = vec![0.0f32; 48_000];
        x[100] = 1.0 / 32_768.0;
        assert!(matches!(classify(&x), SignalClass::VeryLow { .. }));
    }

    #[test]
    fn room_noise_is_present() {
        let x: Vec<f32> = (0..48_000)
            .map(|i| 0.002 * ((i as f32 * 0.37).sin() + (i as f32 * 1.91).sin()))
            .collect();
        let c = classify(&x);
        assert!(c.is_signal(), "{c:?}");
    }

    #[test]
    fn non_finite_samples_count_as_signal() {
        assert!(classify(&[0.0, f32::NAN, 0.0]).is_signal());
    }

    #[test]
    fn peak_meter_zero_is_not_signal() {
        assert!(!peak_is_signal(0.0));
        assert!(!peak_is_signal(f32::NAN));
        assert!(peak_is_signal(1e-6));
    }
}
