//! Python-free application entry points. The migration keeps the legacy package
//! unchanged until its compatibility and installation contracts have Rust parity.
pub mod agent;
pub mod launcher;
pub mod mcp;
pub type Result<T> = std::result::Result<T, String>;
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
