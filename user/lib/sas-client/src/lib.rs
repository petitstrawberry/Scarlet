//! SAS Client Library - Client for the Scarlet Audio Server.
//!
//! This library provides a high-level API for connecting to the Scarlet Audio
//! Server (SAS), configuring an audio stream, and writing PCM samples into a
//! shared-memory ring buffer.
//!
//! # Usage
//!
//! ```no_run
//! use sas_client::{SasClient, StreamConfig};
//! use scarlet_std::audio::AUDIO_PCM_FORMAT_S16LE;
//!
//! let mut client = SasClient::connect()?;
//!
//! let config = StreamConfig {
//!     format: AUDIO_PCM_FORMAT_S16LE,
//!     rate: 48000,
//!     channels: 2,
//!     period_frames: 480,
//!     buffer_frames: 1920,
//! };
//! let mut stream = client.configure(&config)?;
//!
//! // Write interleaved S16LE PCM data
//! stream.write(pcm_bytes);
//!
//! // Wait for the server to drain all buffered audio
//! client.drain()?;
//! client.close()?;
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate scarlet_std as std;

mod client;
mod error;
mod os;
mod stream;

#[cfg(feature = "std")]
#[allow(unused_macros)]
macro_rules! logln {
    ($($arg:tt)*) => {
        std::println!($($arg)*)
    };
}

#[cfg(not(feature = "std"))]
#[allow(unused_macros)]
macro_rules! logln {
    ($($arg:tt)*) => {
        scarlet_std::println!($($arg)*)
    };
}

#[allow(unused_imports)]
pub(crate) use logln;

pub use client::SasClient;
pub use error::Error;
pub use sas_protocol::{ControlState, OutputInfo, OutputRequest};
pub use stream::{SasStream, StreamConfig};
