//! Python-free application entry points. The migration keeps the legacy package
//! unchanged until its compatibility and installation contracts have Rust parity.
pub mod agent;
pub mod calc_worker;
pub mod engine;
pub mod files;
pub mod launcher;
pub mod mcp;
pub mod operations;
pub mod policy;
pub mod transactions;
pub mod uno;
pub mod workflow;
pub type Result<T> = std::result::Result<T, String>;
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
