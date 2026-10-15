//! Platform-neutral audio DSP for the AT2020USB-X control panel.
//!
//! Nothing here touches devices or the OS, so the whole crate builds and tests anywhere.

pub mod denoise;
pub mod dsp;
pub mod offline;
pub mod params;
pub mod resample;
pub mod ring;
pub mod spectrum;
