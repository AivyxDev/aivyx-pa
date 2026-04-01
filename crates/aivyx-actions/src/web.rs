//! Web actions — fetch pages and search the internet.

use crate::Action;
use aivyx_core::Result;

pub struct FetchPage;

#[async_trait::async_trait]
impl Action for FetchPage {
    fn name(&self) -> &str { "fetch_webpage" }

    fn description(&self) -> &str {
        "Fetch a webpage and return its text content"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "URL to fetch" }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let url = input["url"].as_str().unwrap_or_default();

        let response = reqwest::get(url).await.map_err(|e| {
            aivyx_core::AivyxError::Http(e.to_string())
        })?;

        let status = response.status().as_u16();
        let body = response.text().await.map_err(|e| {
            aivyx_core::AivyxError::Http(e.to_string())
        })?;

        // Truncate to avoid blowing up context
        let truncated = if body.len() > 32_000 {
            format!("{}...[truncated]", &body[..32_000])
        } else {
            body
        };

        Ok(serde_json::json!({
            "url": url,
            "status": status,
            "content": truncated,
        }))
    }
}
