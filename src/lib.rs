//! Courseflow platform module: 智慧树 (zhs).
//!
//! This crate is both a library (so the contract tests can drive the platform layer
//! with a mock host) and the `courseflow-zhs` binary the framework spawns.

pub mod error;
pub mod host;
pub mod platform;
pub mod rpc;
pub mod zhs;
