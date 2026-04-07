//! Aivyx Personal Assistant — library interface.
//!
//! Exposes PA modules for integration testing. The binary entry point
//! is in main.rs which re-imports from here.

pub mod agent;
pub mod api;
pub mod config;
pub mod init;
pub mod oauth;
pub mod persona_defaults;
pub mod runtime;
pub mod sessions;
pub mod schedule_tools;
pub mod settings;
pub mod webhook;
