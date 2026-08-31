//! herdash — a terminal dashboard over the herdr agent multiplexer.
//!
//! The library target exists so integration tests in `tests/` can exercise
//! the same code the binary runs.

pub mod app;
pub mod config;
pub mod fleet;
pub mod herdr;
pub mod summary;
pub mod ui;
