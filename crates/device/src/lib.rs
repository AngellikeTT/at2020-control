#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! Windows audio plumbing: Core Audio device control, the capture -> chain -> render
//! engine (shared mode through cpal, exclusive mode through our own WASAPI code) and the
//! recorder tap.

pub mod devctl;
pub mod engine;
pub mod recorder;
pub mod shared;
pub mod wasapi_excl;
pub mod winsys;
