//! Source adapters — each source the loop can check.
//!
//! A source knows how to check for new items and return them as
//! structured data for the briefing or notification system.

use aivyx_core::Result;
use serde::{Deserialize, Serialize};

/// A checkable source of information.
#[async_trait::async_trait]
pub trait Source: Send + Sync {
    /// Human-readable source name.
    fn name(&self) -> &str;

    /// Check for new items since the last check.
    async fn check(&self) -> Result<Vec<SourceItem>>;
}

/// A single item from a source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceItem {
    pub source: String,
    pub title: String,
    pub body: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub urgent: bool,
    pub actionable: bool,
}
