//! Shared protocol definitions and defaults for the Scarlet Tonic demo.

/// Types and services generated from `scarlet_demo.proto`.
pub mod demo {
    tonic::include_proto!("scarlet.demo");
}

/// Default address used by `tonic-server`.
pub const DEFAULT_LISTEN_ADDRESS: &str = "0.0.0.0:50051";

/// Default endpoint used by `tonic-client`.
pub const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:50051";
