//! Aivyx Personal Assistant — library interface.
//!
//! Exposes PA modules for integration testing. The binary entry point
//! is in main.rs which re-imports from here.

pub mod agent;
pub mod api;
pub mod config;
pub mod init;
pub mod oauth;
pub mod passphrase;
pub mod persona_defaults;
pub mod pidfile;
pub mod profile;
pub mod runtime;
pub mod schedule_tools;
pub mod sessions;
pub mod settings;
pub mod webhook;
