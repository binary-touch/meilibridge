// Integration test entry point

#![allow(clippy::collapsible_if)]

#[path = "integration/mod.rs"]
mod integration;

pub use integration::*;
