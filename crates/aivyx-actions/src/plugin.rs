//! Plugin management actions for MCP-based tool packs.
//!
//! Provides tools for the agent to list, install, enable, and disable
//! MCP plugins at runtime. Plugin state is persisted to the encrypted
//! store so changes survive restarts.

use std::sync::Arc;

use aivyx_config::{AivyxConfig, PluginEntry, PluginSource};
use aivyx_core::{AivyxError, Result};
use aivyx_crypto::{EncryptedStore, MasterKey};
use serde::Deserialize;
use tokio::sync::RwLock;
use zeroize::Zeroizing;

use crate::Action;

/// Storage key for persisted plugin state.
const PLUGINS_KEY: &str = "plugins:entries";

/// Shared plugin state — the source of truth for runtime plugin config.
///
/// Wraps the plugins list from `AivyxConfig` in an `RwLock` so the agent
/// can read/mutate it concurrently. Changes are persisted to the encrypted
/// store and survive restarts.
#[derive(Clone)]
pub struct PluginState {
    pub plugins: Arc<RwLock<Vec<PluginEntry>>>,
    store: Arc<EncryptedStore>,
    /// Raw key bytes (MasterKey is not Clone; reconstruct via from_bytes).
    key_bytes: Arc<Zeroizing<[u8; 32]>>,
}

impl PluginState {
    /// Create plugin state from the config's plugin list.
    pub fn new(
        config: &AivyxConfig,
        store: Arc<EncryptedStore>,
        key: MasterKey,
    ) -> Self {
        let key_bytes: [u8; 32] = key.expose_secret().try_into()
            .expect("master key must be 32 bytes");

        // Load persisted plugins, falling back to config file entries.
        let plugins = match store.get(PLUGINS_KEY, &key) {
            Ok(Some(bytes)) => {
                match serde_json::from_slice::<Vec<PluginEntry>>(&bytes) {
                    Ok(entries) => {
                        tracing::info!("Restored {} plugins from encrypted store", entries.len());
                        entries
                    }
                    Err(e) => {
                        tracing::warn!("Failed to deserialize persisted plugins: {e}");
                        config.plugins.clone()
                    }
                }
            }
            _ => config.plugins.clone(),
        };

        Self {
            plugins: Arc::new(RwLock::new(plugins)),
            store,
            key_bytes: Arc::new(Zeroizing::new(key_bytes)),
        }
    }

    /// Reconstruct the domain key from stored bytes.
    fn key(&self) -> MasterKey {
        MasterKey::from_bytes(**self.key_bytes)
    }

    /// Persist the current plugin list to the encrypted store.
    fn persist(&self, plugins: &[PluginEntry]) -> Result<()> {
        let bytes = serde_json::to_vec(plugins)
            .map_err(|e| AivyxError::Other(format!("Failed to serialize plugins: {e}")))?;
        let key = self.key();
        self.store.put(PLUGINS_KEY, &bytes, &key)
            .map_err(|e| AivyxError::Other(format!("Failed to persist plugins: {e}")))
    }

    /// Get enabled plugins (for MCP connection on startup).
    pub async fn enabled_plugins(&self) -> Vec<PluginEntry> {
        self.plugins.read().await.iter().filter(|p| p.enabled).cloned().collect()
    }
}

// ── List Plugins ────────────────────────────────────────────────────

/// List all installed plugins with their status and tool count.
pub struct ListPlugins {
    pub state: PluginState,
}

#[async_trait::async_trait]
impl Action for ListPlugins {
    fn name(&self) -> &str {
        "list_plugins"
    }

    fn description(&self) -> &str {
        "List all installed MCP plugins with their status, version, and description."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "enabled_only": {
                    "type": "boolean",
                    "description": "If true, only show enabled plugins. Default: false."
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let enabled_only = input.get("enabled_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let plugins = self.state.plugins.read().await;
        let entries: Vec<_> = plugins.iter()
            .filter(|p| !enabled_only || p.enabled)
            .map(|p| serde_json::json!({
                "name": p.name,
                "version": p.version,
                "description": p.description,
                "enabled": p.enabled,
                "verified": p.verified,
                "source": match &p.source {
                    PluginSource::Local { path } => format!("local: {path}"),
                    PluginSource::Registry { url } => format!("registry: {url}"),
                },
            }))
            .collect();

        Ok(serde_json::json!({
            "plugins": entries,
            "total": plugins.len(),
            "enabled": plugins.iter().filter(|p| p.enabled).count(),
        }))
    }
}

// ── Enable / Disable Plugin ─────────────────────────────────────────

/// Enable or disable an installed plugin.
pub struct TogglePlugin {
    pub state: PluginState,
    pub enable: bool,
}

#[async_trait::async_trait]
impl Action for TogglePlugin {
    fn name(&self) -> &str {
        if self.enable { "enable_plugin" } else { "disable_plugin" }
    }

    fn description(&self) -> &str {
        if self.enable {
            "Enable a previously disabled MCP plugin. Its tools will become available after restart."
        } else {
            "Disable an MCP plugin. Its tools will be removed after restart. The plugin is not uninstalled."
        }
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The plugin name to enable or disable."
                }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let name = input.get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AivyxError::Other("'name' is required".into()))?;

        let mut plugins = self.state.plugins.write().await;
        let plugin = plugins.iter_mut().find(|p| p.name == name)
            .ok_or_else(|| AivyxError::Other(format!("Plugin '{name}' not found")))?;

        if plugin.enabled == self.enable {
            let state = if self.enable { "enabled" } else { "disabled" };
            return Ok(serde_json::json!({
                "status": "no_change",
                "message": format!("Plugin '{name}' is already {state}."),
            }));
        }

        plugin.enabled = self.enable;
        self.state.persist(&plugins)?;

        let action = if self.enable { "enabled" } else { "disabled" };
        Ok(serde_json::json!({
            "status": "ok",
            "message": format!("Plugin '{name}' {action}. Restart to apply changes."),
        }))
    }
}

// ── Install Plugin ──────────────────────────────────────────────────

/// Install a new plugin from the MCP registry or a local command.
pub struct InstallPlugin {
    pub state: PluginState,
}

/// Parsed install source from user input.
#[derive(Debug, Deserialize)]
struct InstallInput {
    /// Registry name (e.g., "io.github.user/mcp-server") for registry install.
    name: Option<String>,
    /// Local command for stdio transport (e.g., "my-mcp-server").
    command: Option<String>,
    /// Arguments for local command.
    #[serde(default)]
    args: Vec<String>,
    /// Friendly display name (overrides registry name).
    display_name: Option<String>,
}

#[async_trait::async_trait]
impl Action for InstallPlugin {
    fn name(&self) -> &str {
        "install_plugin"
    }

    fn description(&self) -> &str {
        "Install a new MCP plugin. Provide either a registry name to install from the \
         official MCP registry, or a local command for a stdio-based MCP server."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Registry server name (e.g., 'io.github.user/mcp-server'). Searches the official MCP registry."
                },
                "command": {
                    "type": "string",
                    "description": "Local command to run as stdio MCP server (e.g., 'my-mcp-server'). Use this for locally installed servers."
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Arguments for the local command."
                },
                "display_name": {
                    "type": "string",
                    "description": "Friendly name for the plugin (overrides the registry/command name)."
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let parsed: InstallInput = serde_json::from_value(input)
            .map_err(|e| AivyxError::Other(format!("Invalid input: {e}")))?;

        // Must provide either registry name or local command.
        let entry = match (&parsed.name, &parsed.command) {
            (Some(registry_name), None) => {
                self.install_from_registry(registry_name, parsed.display_name.as_deref()).await?
            }
            (None, Some(command)) => {
                self.install_local(command, &parsed.args, parsed.display_name.as_deref())?
            }
            (Some(_), Some(_)) => {
                return Err(AivyxError::Other(
                    "Provide either 'name' (registry) or 'command' (local), not both.".into(),
                ));
            }
            (None, None) => {
                return Err(AivyxError::Other(
                    "Provide either 'name' (registry) or 'command' (local) to install.".into(),
                ));
            }
        };

        // Check for duplicates.
        let mut plugins = self.state.plugins.write().await;
        if plugins.iter().any(|p| p.name == entry.name) {
            return Err(AivyxError::Other(format!(
                "Plugin '{}' is already installed. Use enable_plugin to re-enable it.",
                entry.name,
            )));
        }

        let summary = serde_json::json!({
            "name": entry.name,
            "version": entry.version,
            "description": entry.description,
            "source": match &entry.source {
                PluginSource::Local { path } => format!("local: {path}"),
                PluginSource::Registry { url } => format!("registry: {url}"),
            },
        });

        plugins.push(entry);
        self.state.persist(&plugins)?;

        Ok(serde_json::json!({
            "status": "installed",
            "plugin": summary,
            "message": "Plugin installed. Restart to connect and discover its tools.",
        }))
    }
}

impl InstallPlugin {
    async fn install_from_registry(
        &self,
        registry_name: &str,
        display_name: Option<&str>,
    ) -> Result<PluginEntry> {
        use aivyx_mcp::registry::McpRegistryClient;

        let client = McpRegistryClient::new();
        let server = client.get(registry_name).await?;
        let mcp_config = McpRegistryClient::to_mcp_config(&server)?;

        let name = display_name.unwrap_or(&mcp_config.name).to_string();

        Ok(PluginEntry {
            name,
            version: server.version.clone(),
            description: server.description.clone(),
            author: server.repository.as_ref().map(|r| r.url.clone()),
            source: PluginSource::Registry {
                url: format!("https://registry.modelcontextprotocol.io/v0.1/servers/{registry_name}"),
            },
            mcp_config,
            installed_at: chrono::Utc::now(),
            enabled: true,
            verified: false,
            security_notes: None,
        })
    }

    fn install_local(
        &self,
        command: &str,
        args: &[String],
        display_name: Option<&str>,
    ) -> Result<PluginEntry> {
        use aivyx_config::{McpServerConfig, McpTransport};
        use std::collections::HashMap;

        let name = display_name.unwrap_or(command).to_string();

        let mcp_config = McpServerConfig {
            name: name.clone(),
            transport: McpTransport::Stdio {
                command: command.to_string(),
                args: args.to_vec(),
            },
            env: HashMap::new(),
            timeout_secs: 30,
            allowed_tools: None,
            blocked_tools: None,
            max_reconnect_attempts: 3,
            reconnect_backoff_ms: 1000,
            tool_timeouts: HashMap::new(),
            tool_rate_limits: HashMap::new(),
            tool_cache_ttls: HashMap::new(),
            tool_costs: HashMap::new(),
            keepalive_interval_secs: 30,
            sandbox: None,
            auto_disable_threshold: None,
            min_calls_for_disable: 20,
        };

        Ok(PluginEntry {
            name,
            version: "local".to_string(),
            description: format!("Local MCP server: {command}"),
            author: None,
            source: PluginSource::Local {
                path: command.to_string(),
            },
            mcp_config,
            installed_at: chrono::Utc::now(),
            enabled: true,
            verified: false,
            security_notes: None,
        })
    }
}

// ── Uninstall Plugin ────────────────────────────────────────────────

/// Remove an installed plugin completely.
pub struct UninstallPlugin {
    pub state: PluginState,
}

#[async_trait::async_trait]
impl Action for UninstallPlugin {
    fn name(&self) -> &str {
        "uninstall_plugin"
    }

    fn description(&self) -> &str {
        "Uninstall an MCP plugin completely. This removes the plugin from the configuration. \
         Its tools will no longer be available after restart."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The plugin name to uninstall."
                }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let name = input.get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AivyxError::Other("'name' is required".into()))?;

        let mut plugins = self.state.plugins.write().await;
        let idx = plugins.iter().position(|p| p.name == name)
            .ok_or_else(|| AivyxError::Other(format!("Plugin '{name}' not found")))?;

        let removed = plugins.remove(idx);
        self.state.persist(&plugins)?;

        Ok(serde_json::json!({
            "status": "uninstalled",
            "message": format!("Plugin '{}' v{} removed.", removed.name, removed.version),
        }))
    }
}

// ── Search Registry ─────────────────────────────────────────────────

/// Search the official MCP registry for available plugins.
pub struct SearchPluginRegistry;

#[async_trait::async_trait]
impl Action for SearchPluginRegistry {
    fn name(&self) -> &str {
        "search_plugins"
    }

    fn description(&self) -> &str {
        "Search the official MCP plugin registry for available tools and integrations. \
         Returns matching servers with their descriptions and install info."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query (e.g., 'github', 'filesystem', 'slack')."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum results to return. Default: 10."
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let query = input.get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AivyxError::Other("'query' is required".into()))?;
        let limit = input.get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as u32;

        let client = aivyx_mcp::registry::McpRegistryClient::new();
        let results = client.search(query, limit).await?;

        let entries: Vec<_> = results.iter().map(|s| {
            serde_json::json!({
                "name": s.name,
                "description": s.description,
                "version": s.version,
                "has_packages": !s.packages.is_empty(),
                "has_remotes": !s.remotes.is_empty(),
            })
        }).collect();

        Ok(serde_json::json!({
            "results": entries,
            "count": entries.len(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivyx_config::{McpServerConfig, McpTransport};
    use std::collections::HashMap;

    fn mock_config_with_plugins() -> AivyxConfig {
        let mut config = AivyxConfig::default();
        config.plugins.push(PluginEntry {
            name: "test-plugin".into(),
            version: "1.0.0".into(),
            description: "A test plugin".into(),
            author: None,
            source: PluginSource::Local { path: "/usr/bin/test-mcp".into() },
            mcp_config: McpServerConfig {
                name: "test-plugin".into(),
                transport: McpTransport::Stdio {
                    command: "test-mcp".into(),
                    args: vec![],
                },
                env: HashMap::new(),
                timeout_secs: 30,
                allowed_tools: None,
                blocked_tools: None,
                max_reconnect_attempts: 3,
                reconnect_backoff_ms: 1000,
                tool_timeouts: HashMap::new(),
                tool_rate_limits: HashMap::new(),
                tool_cache_ttls: HashMap::new(),
                tool_costs: HashMap::new(),
                keepalive_interval_secs: 30,
                sandbox: None,
                auto_disable_threshold: None,
                min_calls_for_disable: 20,
            },
            installed_at: chrono::Utc::now(),
            enabled: true,
            verified: false,
            security_notes: None,
        });
        config
    }

    fn temp_store() -> (Arc<EncryptedStore>, MasterKey) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(EncryptedStore::open(dir.path().join("test.db")).unwrap());
        let key = MasterKey::from_bytes([0u8; 32]);
        (store, key)
    }

    #[tokio::test]
    async fn list_plugins_returns_all() {
        let config = mock_config_with_plugins();
        let (store, key) = temp_store();
        let state = PluginState::new(&config, store, key);
        let action = ListPlugins { state };

        let result = action.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(result["total"], 1);
        assert_eq!(result["enabled"], 1);
        assert_eq!(result["plugins"][0]["name"], "test-plugin");
    }

    #[tokio::test]
    async fn toggle_plugin_disable() {
        let config = mock_config_with_plugins();
        let (store, key) = temp_store();
        let state = PluginState::new(&config, store, key);
        let action = TogglePlugin { state: state.clone(), enable: false };

        let result = action.execute(serde_json::json!({"name": "test-plugin"})).await.unwrap();
        assert_eq!(result["status"], "ok");

        // Verify it's disabled.
        let plugins = state.plugins.read().await;
        assert!(!plugins[0].enabled);
    }

    #[tokio::test]
    async fn toggle_plugin_not_found() {
        let config = AivyxConfig::default();
        let (store, key) = temp_store();
        let state = PluginState::new(&config, store, key);
        let action = TogglePlugin { state, enable: true };

        let result = action.execute(serde_json::json!({"name": "nonexistent"})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn install_local_plugin() {
        let config = AivyxConfig::default();
        let (store, key) = temp_store();
        let state = PluginState::new(&config, store, key);
        let action = InstallPlugin { state: state.clone() };

        let result = action.execute(serde_json::json!({
            "command": "my-mcp-server",
            "args": ["--port", "3000"],
            "display_name": "my-server"
        })).await.unwrap();

        assert_eq!(result["status"], "installed");
        assert_eq!(result["plugin"]["name"], "my-server");

        let plugins = state.plugins.read().await;
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "my-server");
    }

    #[tokio::test]
    async fn install_duplicate_fails() {
        let config = mock_config_with_plugins();
        let (store, key) = temp_store();
        let state = PluginState::new(&config, store, key);
        let action = InstallPlugin { state };

        let result = action.execute(serde_json::json!({
            "command": "test-mcp",
            "display_name": "test-plugin"
        })).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn uninstall_plugin() {
        let config = mock_config_with_plugins();
        let (store, key) = temp_store();
        let state = PluginState::new(&config, store, key);
        let action = UninstallPlugin { state: state.clone() };

        let result = action.execute(serde_json::json!({"name": "test-plugin"})).await.unwrap();
        assert_eq!(result["status"], "uninstalled");

        let plugins = state.plugins.read().await;
        assert!(plugins.is_empty());
    }

    #[tokio::test]
    async fn enabled_plugins_filters() {
        let mut config = mock_config_with_plugins();
        config.plugins[0].enabled = false;
        config.plugins.push(PluginEntry {
            name: "active-plugin".into(),
            version: "2.0.0".into(),
            description: "Active".into(),
            author: None,
            source: PluginSource::Local { path: "/usr/bin/active-mcp".into() },
            mcp_config: config.plugins[0].mcp_config.clone(),
            installed_at: chrono::Utc::now(),
            enabled: true,
            verified: false,
            security_notes: None,
        });
        let (store, key) = temp_store();
        let state = PluginState::new(&config, store, key);

        let enabled = state.enabled_plugins().await;
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].name, "active-plugin");
    }
}
