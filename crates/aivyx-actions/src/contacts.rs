//! Contact management — CardDAV client for syncing and resolving contacts.
//!
//! Supports any CardDAV-compliant server (Google Contacts, Nextcloud,
//! iCloud, Radicale, etc.) via standard PROPFIND/REPORT requests.
//!
//! Contacts are also stored locally in the encrypted store for fast
//! resolution without network round-trips.

use aivyx_core::Result;
use aivyx_crypto::{EncryptedStore, MasterKey};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::Action;

// ── Configuration ─────────────────────────────────────────────────

/// CardDAV connection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactsConfig {
    /// CardDAV server URL (e.g., "https://contacts.example.com/dav/addressbooks/user/")
    pub url: String,
    /// Username for Basic auth.
    pub username: String,
    /// Password for Basic auth — loaded from encrypted keystore at runtime.
    #[serde(skip)]
    pub password: String,
    /// Optional specific address book path (auto-discovered if not set).
    pub addressbook_path: Option<String>,
}

// ── Data types ────────────────────────────────────────────────────

/// A contact parsed from vCard data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contact {
    /// Unique identifier (from UID property or generated).
    pub uid: String,
    /// Full display name.
    pub name: String,
    /// First name (given name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_name: Option<String>,
    /// Last name (family name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_name: Option<String>,
    /// Email addresses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub emails: Vec<String>,
    /// Phone numbers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phones: Vec<String>,
    /// Organization / company name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company: Option<String>,
    /// Job title / role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Free-form notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Source: "carddav", "email-enrichment", "manual"
    #[serde(default = "default_source")]
    pub source: String,
}

fn default_source() -> String {
    "manual".into()
}

/// Key prefix for contacts in the encrypted store.
const CONTACT_PREFIX: &str = "contact:";

// ── Local contact store ──────────────────────────────────────────

/// Save a contact to the encrypted store.
pub fn save_contact(
    store: &EncryptedStore,
    key: &MasterKey,
    contact: &Contact,
) -> Result<()> {
    let json = serde_json::to_vec(contact)
        .map_err(aivyx_core::AivyxError::Serialization)?;
    store.put(&format!("{CONTACT_PREFIX}{}", contact.uid), &json, key)
}

/// Load all contacts from the encrypted store.
pub fn load_all_contacts(
    store: &EncryptedStore,
    key: &MasterKey,
) -> Result<Vec<Contact>> {
    let keys = store.list_keys()?;
    let mut contacts = Vec::new();

    for store_key in &keys {
        if !store_key.starts_with(CONTACT_PREFIX) {
            continue;
        }
        if let Some(bytes) = store.get(store_key, key)? {
            match serde_json::from_slice::<Contact>(&bytes) {
                Ok(c) => contacts.push(c),
                Err(e) => tracing::warn!("Corrupt contact entry '{store_key}': {e}"),
            }
        }
    }

    contacts.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(contacts)
}

/// Resolve a contact by fuzzy name or email match.
///
/// Search is case-insensitive and matches against:
/// - Full name, first name, last name
/// - Email addresses
/// - Company name
///
/// Returns all contacts where any field contains the query.
pub fn resolve_contact(
    store: &EncryptedStore,
    key: &MasterKey,
    query: &str,
) -> Result<Vec<Contact>> {
    let all = load_all_contacts(store, key)?;
    let q = query.to_lowercase();

    Ok(all
        .into_iter()
        .filter(|c| {
            c.name.to_lowercase().contains(&q)
                || c.first_name.as_ref().is_some_and(|n| n.to_lowercase().contains(&q))
                || c.last_name.as_ref().is_some_and(|n| n.to_lowercase().contains(&q))
                || c.emails.iter().any(|e| e.to_lowercase().contains(&q))
                || c.company.as_ref().is_some_and(|co| co.to_lowercase().contains(&q))
        })
        .collect())
}

// ── CardDAV client ───────────────────────────────────────────────

/// Fetch contacts from a CardDAV server.
pub async fn fetch_contacts(config: &ContactsConfig) -> Result<Vec<Contact>> {
    let client = crate::http_client();

    let addressbook_url = if let Some(ref path) = config.addressbook_path {
        if path.starts_with("http") {
            path.clone()
        } else {
            format!("{}{}", config.url.trim_end_matches('/'), path)
        }
    } else {
        discover_addressbook(client, config).await?
    };

    // REPORT: addressbook-query to fetch all vCards.
    let report_body = build_addressbook_query_xml();

    let response = crate::retry::retry(
        &crate::retry::RetryConfig::network(),
        || async {
            client
                .request(
                    reqwest::Method::from_bytes(b"REPORT").unwrap(),
                    &addressbook_url,
                )
                .basic_auth(&config.username, Some(&config.password))
                .header("Depth", "1")
                .header("Content-Type", "application/xml; charset=utf-8")
                .body(report_body.clone())
                .send()
                .await
                .map_err(|e| aivyx_core::AivyxError::Http(e.to_string()))
        },
        crate::retry::is_transient,
    )
    .await?;

    let status = response.status();
    if !status.is_success() && status.as_u16() != 207 {
        return Err(aivyx_core::AivyxError::Http(format!(
            "CardDAV REPORT failed: HTTP {status}"
        )));
    }

    let body = response
        .text()
        .await
        .map_err(|e| aivyx_core::AivyxError::Http(e.to_string()))?;

    parse_multistatus_contacts(&body)
}

/// Discover the first address book URL via PROPFIND.
async fn discover_addressbook(
    client: &reqwest::Client,
    config: &ContactsConfig,
) -> Result<String> {
    let propfind_body = r#"<?xml version="1.0" encoding="UTF-8"?>
<d:propfind xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
  <d:prop>
    <d:resourcetype/>
    <d:displayname/>
  </d:prop>
</d:propfind>"#;

    let response = client
        .request(
            reqwest::Method::from_bytes(b"PROPFIND").unwrap(),
            &config.url,
        )
        .basic_auth(&config.username, Some(&config.password))
        .header("Depth", "1")
        .header("Content-Type", "application/xml; charset=utf-8")
        .body(propfind_body)
        .send()
        .await
        .map_err(|e| aivyx_core::AivyxError::Http(format!("CardDAV PROPFIND failed: {e}")))?;

    let body = response
        .text()
        .await
        .map_err(|e| aivyx_core::AivyxError::Http(e.to_string()))?;

    let doc = roxmltree::Document::parse(&body).map_err(|e| {
        aivyx_core::AivyxError::Http(format!("CardDAV XML parse error: {e}"))
    })?;

    // Look for <response> elements that have <resourcetype><addressbook/>.
    for response_node in doc.descendants().filter(|n| n.has_tag_name("response")) {
        let is_addressbook = response_node
            .descendants()
            .any(|n| n.has_tag_name("addressbook"));
        if is_addressbook
            && let Some(href_node) = response_node.descendants().find(|n| n.has_tag_name("href"))
                && let Some(href) = href_node.text() {
                    return Ok(resolve_url(&config.url, href));
                }
    }

    Err(aivyx_core::AivyxError::Http(
        "No address book found via PROPFIND discovery".into(),
    ))
}

/// Build the XML body for a CardDAV addressbook-query REPORT.
fn build_addressbook_query_xml() -> String {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<card:addressbook-query xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
  <d:prop>
    <d:getetag/>
    <card:address-data/>
  </d:prop>
</card:addressbook-query>"#
        .to_string()
}

use crate::resolve_url;

// ── vCard parsing ────────────────────────────────────────────────

/// Parse a CardDAV multistatus XML response and extract vCard contacts.
fn parse_multistatus_contacts(xml: &str) -> Result<Vec<Contact>> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| {
        aivyx_core::AivyxError::Http(format!("CardDAV XML parse error: {e}"))
    })?;

    let mut contacts = Vec::new();

    for node in doc.descendants() {
        if node.has_tag_name("address-data")
            && let Some(vcard_text) = node.text()
                && let Some(contact) = parse_vcard(vcard_text) {
                    contacts.push(contact);
                }
    }

    contacts.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(contacts)
}

/// Parse a single vCard text into a Contact.
///
/// Handles vCard 3.0 and 4.0 formats. Extracts:
/// - FN (display name)
/// - N (structured name → first/last)
/// - EMAIL
/// - TEL
/// - ORG
/// - TITLE
/// - NOTE
/// - UID
fn parse_vcard(text: &str) -> Option<Contact> {
    let mut uid = None;
    let mut fn_name = None;
    let mut first_name = None;
    let mut last_name = None;
    let mut emails = Vec::new();
    let mut phones = Vec::new();
    let mut company = None;
    let mut title = None;
    let mut notes = None;

    for line in unfold_vcard_lines(text) {
        // Split on first ':' to get property name (with params) and value.
        let Some((prop_with_params, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }

        // Property name is everything before the first ';' or ':'.
        let prop = prop_with_params
            .split(';')
            .next()
            .unwrap_or(prop_with_params)
            .to_uppercase();

        match prop.as_str() {
            "UID" => uid = Some(value.to_string()),
            "FN" => fn_name = Some(value.to_string()),
            "N" => {
                // N:Last;First;Middle;Prefix;Suffix
                let parts: Vec<&str> = value.split(';').collect();
                if let Some(last) = parts.first() {
                    let last = last.trim();
                    if !last.is_empty() {
                        last_name = Some(last.to_string());
                    }
                }
                if let Some(first) = parts.get(1) {
                    let first = first.trim();
                    if !first.is_empty() {
                        first_name = Some(first.to_string());
                    }
                }
            }
            "EMAIL" => emails.push(value.to_string()),
            "TEL" => phones.push(value.to_string()),
            "ORG" => {
                // ORG may have sub-units separated by ';' — take the first.
                let org = value.split(';').next().unwrap_or(value).trim();
                if !org.is_empty() {
                    company = Some(org.to_string());
                }
            }
            "TITLE" => title = Some(value.to_string()),
            "NOTE" => notes = Some(value.to_string()),
            _ => {}
        }
    }

    // FN is required by the vCard spec; skip entries without it.
    let name = fn_name?;

    Some(Contact {
        uid: uid.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        name,
        first_name,
        last_name,
        emails,
        phones,
        company,
        title,
        notes,
        source: "carddav".into(),
    })
}

/// Unfold vCard continuation lines (lines starting with a space or tab
/// are continuations of the previous line per RFC 6350 §3.2).
fn unfold_vcard_lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for raw_line in text.lines() {
        if raw_line.starts_with(' ') || raw_line.starts_with('\t') {
            // Continuation: append to previous line (strip the single fold indicator).
            if let Some(last) = lines.last_mut() {
                let last: &mut String = last;
                last.push_str(&raw_line[1..]);
            }
        } else {
            lines.push(raw_line.to_string());
        }
    }
    lines
}

// ── Email enrichment ─────────────────────────────────────────────

/// Try to enrich/create a contact from an email "From" header.
///
/// Parses `"Jane Doe <jane@example.com>"` and creates or updates a contact
/// if one doesn't already exist for that email address.
///
/// Returns `Some(contact)` if a new contact was created or updated.
pub fn enrich_from_email_header(
    store: &EncryptedStore,
    key: &MasterKey,
    from_header: &str,
) -> Result<Option<Contact>> {
    let (name, email) = parse_email_from(from_header);
    let Some(email) = email else {
        return Ok(None);
    };

    // Check if we already have a contact with this email.
    let existing = load_all_contacts(store, key)?;
    let already_exists = existing.iter().any(|c| {
        c.emails.iter().any(|e| e.eq_ignore_ascii_case(&email))
    });

    if already_exists {
        return Ok(None);
    }

    // Create a new contact from the email header.
    let display_name = name.unwrap_or_else(|| email.clone());
    let (first, last) = split_name(&display_name);

    let contact = Contact {
        uid: uuid::Uuid::new_v4().to_string(),
        name: display_name,
        first_name: first,
        last_name: last,
        emails: vec![email],
        phones: vec![],
        company: None,
        title: None,
        notes: None,
        source: "email-enrichment".into(),
    };

    save_contact(store, key, &contact)?;
    tracing::info!("Auto-created contact '{}' from email header", contact.name);

    Ok(Some(contact))
}

/// Parse an RFC 5322 From header: `"Name" <email>` or `email`.
fn parse_email_from(header: &str) -> (Option<String>, Option<String>) {
    let header = header.trim();

    // Try "Name <email@domain>" format
    if let Some(angle_start) = header.rfind('<')
        && let Some(angle_end) = header.rfind('>') {
            let email = header[angle_start + 1..angle_end].trim().to_string();
            let name_part = header[..angle_start].trim();
            // Strip quotes around name
            let name = name_part.trim_matches('"').trim();
            let name = if name.is_empty() { None } else { Some(name.to_string()) };
            return (name, Some(email));
        }

    // Plain email address
    if header.contains('@') {
        return (None, Some(header.to_string()));
    }

    (None, None)
}

/// Best-effort split of a display name into first/last.
fn split_name(name: &str) -> (Option<String>, Option<String>) {
    let parts: Vec<&str> = name.split_whitespace().collect();
    match parts.len() {
        0 => (None, None),
        1 => (Some(parts[0].to_string()), None),
        _ => {
            let first = parts[0].to_string();
            let last = parts[1..].join(" ");
            (Some(first), Some(last))
        }
    }
}

// ── Action tools ─────────────────────────────────────────────────

/// Tool: search contacts by name, email, or company.
pub struct SearchContacts {
    pub store: Arc<EncryptedStore>,
    pub key: MasterKey,
}

#[async_trait::async_trait]
impl Action for SearchContacts {
    fn name(&self) -> &str {
        "search_contacts"
    }

    fn description(&self) -> &str {
        "Search contacts by name, email address, or company. Returns matching contacts \
         with their email addresses, phone numbers, and other details."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Name, email, or company to search for"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let query = input["query"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'query' is required".into()))?;

        let matches = resolve_contact(&self.store, &self.key, query)?;
        Ok(serde_json::to_value(&matches)?)
    }
}

/// Tool: list all contacts.
pub struct ListContacts {
    pub store: Arc<EncryptedStore>,
    pub key: MasterKey,
}

#[async_trait::async_trait]
impl Action for ListContacts {
    fn name(&self) -> &str {
        "list_contacts"
    }

    fn description(&self) -> &str {
        "List all saved contacts. Use search_contacts to find a specific person."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of contacts to return (default: 50)"
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let limit = input["limit"].as_u64().unwrap_or(50) as usize;
        let all = load_all_contacts(&self.store, &self.key)?;
        let truncated: Vec<&Contact> = all.iter().take(limit).collect();
        Ok(serde_json::to_value(&truncated)?)
    }
}

/// Tool: sync contacts from CardDAV server into local store.
pub struct SyncContacts {
    pub config: ContactsConfig,
    pub store: Arc<EncryptedStore>,
    pub key: MasterKey,
}

#[async_trait::async_trait]
impl Action for SyncContacts {
    fn name(&self) -> &str {
        "sync_contacts"
    }

    fn description(&self) -> &str {
        "Sync contacts from the CardDAV server into the local store. \
         New contacts are added; existing ones (by UID) are updated."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, _input: serde_json::Value) -> Result<serde_json::Value> {
        let remote = fetch_contacts(&self.config).await?;
        let mut added = 0u32;
        let mut updated = 0u32;

        let existing = load_all_contacts(&self.store, &self.key)?;
        let existing_uids: std::collections::HashSet<&str> =
            existing.iter().map(|c| c.uid.as_str()).collect();

        for contact in &remote {
            if existing_uids.contains(contact.uid.as_str()) {
                updated += 1;
            } else {
                added += 1;
            }
            save_contact(&self.store, &self.key, contact)?;
        }

        tracing::info!("Contact sync: {added} added, {updated} updated ({} total)", remote.len());

        Ok(serde_json::json!({
            "status": "synced",
            "total": remote.len(),
            "added": added,
            "updated": updated,
        }))
    }
}

// ── CUD tools ──────────────────────────────────────────────────

/// Tool: add a contact manually.
pub struct AddContact {
    pub store: Arc<EncryptedStore>,
    pub key: MasterKey,
}

#[async_trait::async_trait]
impl Action for AddContact {
    fn name(&self) -> &str {
        "add_contact"
    }

    fn description(&self) -> &str {
        "Add a new contact manually. Provide at least the full name."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": { "type": "string", "description": "Full display name" },
                "first_name": { "type": "string" },
                "last_name": { "type": "string" },
                "emails": { "type": "array", "items": { "type": "string" }, "description": "Email addresses" },
                "phones": { "type": "array", "items": { "type": "string" }, "description": "Phone numbers" },
                "company": { "type": "string" },
                "title": { "type": "string", "description": "Job title" },
                "notes": { "type": "string" }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let name = input["name"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'name' is required".into()))?;

        let uid = uuid::Uuid::new_v4().to_string();
        let contact = Contact {
            uid: uid.clone(),
            name: name.to_string(),
            first_name: input["first_name"].as_str().map(String::from),
            last_name: input["last_name"].as_str().map(String::from),
            emails: input["emails"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default(),
            phones: input["phones"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default(),
            company: input["company"].as_str().map(String::from),
            title: input["title"].as_str().map(String::from),
            notes: input["notes"].as_str().map(String::from),
            source: "manual".into(),
        };

        save_contact(&self.store, &self.key, &contact)?;

        Ok(serde_json::json!({
            "status": "created",
            "uid": uid,
            "name": contact.name,
        }))
    }
}

/// Tool: update an existing contact.
pub struct UpdateContact {
    pub store: Arc<EncryptedStore>,
    pub key: MasterKey,
}

#[async_trait::async_trait]
impl Action for UpdateContact {
    fn name(&self) -> &str {
        "update_contact"
    }

    fn description(&self) -> &str {
        "Update an existing contact. Only the fields you provide will be changed."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["uid"],
            "properties": {
                "uid": { "type": "string", "description": "Contact UID to update" },
                "name": { "type": "string" },
                "first_name": { "type": "string" },
                "last_name": { "type": "string" },
                "emails": { "type": "array", "items": { "type": "string" } },
                "phones": { "type": "array", "items": { "type": "string" } },
                "company": { "type": "string" },
                "title": { "type": "string" },
                "notes": { "type": "string" }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let uid = input["uid"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'uid' is required".into()))?;

        let store_key = format!("{CONTACT_PREFIX}{uid}");
        let bytes = self.store.get(&store_key, &self.key)?
            .ok_or_else(|| aivyx_core::AivyxError::Validation(format!("Contact '{uid}' not found")))?;
        let mut contact: Contact = serde_json::from_slice(&bytes)
            .map_err(|e| aivyx_core::AivyxError::Other(format!("Corrupt contact: {e}")))?;

        if let Some(v) = input["name"].as_str() { contact.name = v.into(); }
        if !input["first_name"].is_null() { contact.first_name = input["first_name"].as_str().map(String::from); }
        if !input["last_name"].is_null() { contact.last_name = input["last_name"].as_str().map(String::from); }
        if let Some(a) = input["emails"].as_array() {
            contact.emails = a.iter().filter_map(|v| v.as_str().map(String::from)).collect();
        }
        if let Some(a) = input["phones"].as_array() {
            contact.phones = a.iter().filter_map(|v| v.as_str().map(String::from)).collect();
        }
        if !input["company"].is_null() { contact.company = input["company"].as_str().map(String::from); }
        if !input["title"].is_null() { contact.title = input["title"].as_str().map(String::from); }
        if !input["notes"].is_null() { contact.notes = input["notes"].as_str().map(String::from); }

        save_contact(&self.store, &self.key, &contact)?;

        Ok(serde_json::json!({
            "status": "updated",
            "uid": uid,
        }))
    }
}

/// Tool: delete a contact by UID.
pub struct DeleteContact {
    pub store: Arc<EncryptedStore>,
    pub key: MasterKey,
}

#[async_trait::async_trait]
impl Action for DeleteContact {
    fn name(&self) -> &str {
        "delete_contact"
    }

    fn description(&self) -> &str {
        "Permanently delete a contact by UID."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["uid"],
            "properties": {
                "uid": { "type": "string", "description": "Contact UID to delete" }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let uid = input["uid"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'uid' is required".into()))?;

        let store_key = format!("{CONTACT_PREFIX}{uid}");
        // Verify it exists before deleting
        if self.store.get(&store_key, &self.key)?.is_none() {
            return Err(aivyx_core::AivyxError::Validation(format!("Contact '{uid}' not found")));
        }

        self.store.delete(&store_key)?;

        Ok(serde_json::json!({
            "status": "deleted",
            "uid": uid,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aivyx_crypto::MasterKey;

    fn setup() -> (Arc<EncryptedStore>, MasterKey, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "aivyx-contacts-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = EncryptedStore::open(dir.join("store.db")).unwrap();
        let key = MasterKey::generate();
        (Arc::new(store), key, dir)
    }

    fn cleanup(dir: std::path::PathBuf) {
        let _ = std::fs::remove_dir_all(dir);
    }

    fn contacts_key(key: &MasterKey) -> MasterKey {
        aivyx_crypto::derive_domain_key(key, b"contacts")
    }

    // ── vCard parsing tests ──────────────────────────────────────

    #[test]
    fn parse_vcard_full() {
        let vcard = "BEGIN:VCARD\r\n\
            VERSION:3.0\r\n\
            UID:abc-123\r\n\
            FN:Sarah Jones\r\n\
            N:Jones;Sarah;;;\r\n\
            EMAIL;TYPE=WORK:sarah@acme.com\r\n\
            EMAIL;TYPE=HOME:sarah.j@gmail.com\r\n\
            TEL;TYPE=CELL:+1-555-0123\r\n\
            ORG:Acme Corp\r\n\
            TITLE:VP of Engineering\r\n\
            NOTE:Met at the conference\r\n\
            END:VCARD";

        let contact = parse_vcard(vcard).unwrap();
        assert_eq!(contact.uid, "abc-123");
        assert_eq!(contact.name, "Sarah Jones");
        assert_eq!(contact.first_name.as_deref(), Some("Sarah"));
        assert_eq!(contact.last_name.as_deref(), Some("Jones"));
        assert_eq!(contact.emails, vec!["sarah@acme.com", "sarah.j@gmail.com"]);
        assert_eq!(contact.phones, vec!["+1-555-0123"]);
        assert_eq!(contact.company.as_deref(), Some("Acme Corp"));
        assert_eq!(contact.title.as_deref(), Some("VP of Engineering"));
        assert_eq!(contact.notes.as_deref(), Some("Met at the conference"));
        assert_eq!(contact.source, "carddav");
    }

    #[test]
    fn parse_vcard_minimal() {
        let vcard = "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Alice\r\nEND:VCARD";
        let contact = parse_vcard(vcard).unwrap();
        assert_eq!(contact.name, "Alice");
        assert!(contact.emails.is_empty());
        assert!(contact.last_name.is_none());
    }

    #[test]
    fn parse_vcard_without_fn_returns_none() {
        let vcard = "BEGIN:VCARD\r\nVERSION:3.0\r\nEMAIL:nobody@test.com\r\nEND:VCARD";
        assert!(parse_vcard(vcard).is_none());
    }

    #[test]
    fn parse_vcard_folded_lines() {
        // RFC 6350 §3.2: continuation lines start with a single space (fold indicator).
        // The fold indicator is stripped; the rest is appended directly.
        // Real vCard generators fold at word boundaries, so the previous line
        // typically ends with a space before the fold.
        let vcard = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:A Very Long Name \r\n That Gets Folded\r\nEND:VCARD";
        let contact = parse_vcard(vcard).unwrap();
        assert_eq!(contact.name, "A Very Long Name That Gets Folded");
    }

    #[test]
    fn parse_vcard_org_with_subunits() {
        let vcard = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Bob\r\n\
            ORG:Acme Corp;Engineering;Platform\r\nEND:VCARD";
        let contact = parse_vcard(vcard).unwrap();
        assert_eq!(contact.company.as_deref(), Some("Acme Corp"));
    }

    #[test]
    fn parse_multistatus_extracts_contacts() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
  <d:response>
    <d:href>/contacts/alice.vcf</d:href>
    <d:propstat>
      <d:prop>
        <card:address-data>BEGIN:VCARD
VERSION:3.0
UID:alice-1
FN:Alice Wonderland
EMAIL:alice@example.com
END:VCARD</card:address-data>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#;

        let contacts = parse_multistatus_contacts(xml).unwrap();
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].name, "Alice Wonderland");
        assert_eq!(contacts[0].emails, vec!["alice@example.com"]);
    }

    #[test]
    fn parse_multistatus_empty() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
</d:multistatus>"#;
        let contacts = parse_multistatus_contacts(xml).unwrap();
        assert!(contacts.is_empty());
    }

    // ── Email header parsing tests ───────────────────────────────

    #[test]
    fn parse_email_from_full() {
        let (name, email) = parse_email_from("\"Jane Doe\" <jane@example.com>");
        assert_eq!(name.as_deref(), Some("Jane Doe"));
        assert_eq!(email.as_deref(), Some("jane@example.com"));
    }

    #[test]
    fn parse_email_from_no_quotes() {
        let (name, email) = parse_email_from("Jane Doe <jane@example.com>");
        assert_eq!(name.as_deref(), Some("Jane Doe"));
        assert_eq!(email.as_deref(), Some("jane@example.com"));
    }

    #[test]
    fn parse_email_from_bare_address() {
        let (name, email) = parse_email_from("jane@example.com");
        assert!(name.is_none());
        assert_eq!(email.as_deref(), Some("jane@example.com"));
    }

    #[test]
    fn parse_email_from_no_name() {
        let (name, email) = parse_email_from("<jane@example.com>");
        assert!(name.is_none());
        assert_eq!(email.as_deref(), Some("jane@example.com"));
    }

    #[test]
    fn parse_email_from_garbage() {
        let (name, email) = parse_email_from("not an email");
        assert!(name.is_none());
        assert!(email.is_none());
    }

    // ── Split name tests ─────────────────────────────────────────

    #[test]
    fn split_name_two_parts() {
        let (first, last) = split_name("Jane Doe");
        assert_eq!(first.as_deref(), Some("Jane"));
        assert_eq!(last.as_deref(), Some("Doe"));
    }

    #[test]
    fn split_name_three_parts() {
        let (first, last) = split_name("Jane Van Doe");
        assert_eq!(first.as_deref(), Some("Jane"));
        assert_eq!(last.as_deref(), Some("Van Doe"));
    }

    #[test]
    fn split_name_single() {
        let (first, last) = split_name("Madonna");
        assert_eq!(first.as_deref(), Some("Madonna"));
        assert!(last.is_none());
    }

    // ── Contact store tests ──────────────────────────────────────

    #[test]
    fn save_and_load_contacts() {
        let (store, key, dir) = setup();
        let ck = contacts_key(&key);

        let contact = Contact {
            uid: "test-1".into(),
            name: "Sarah Jones".into(),
            first_name: Some("Sarah".into()),
            last_name: Some("Jones".into()),
            emails: vec!["sarah@acme.com".into()],
            phones: vec![],
            company: Some("Acme Corp".into()),
            title: None,
            notes: None,
            source: "manual".into(),
        };

        save_contact(&store, &ck, &contact).unwrap();
        let all = load_all_contacts(&store, &ck).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Sarah Jones");
        assert_eq!(all[0].company.as_deref(), Some("Acme Corp"));

        cleanup(dir);
    }

    #[test]
    fn resolve_contact_by_first_name() {
        let (store, key, dir) = setup();
        let ck = contacts_key(&key);

        save_contact(&store, &ck, &Contact {
            uid: "c1".into(), name: "Sarah Jones".into(),
            first_name: Some("Sarah".into()), last_name: Some("Jones".into()),
            emails: vec!["sarah@acme.com".into()],
            phones: vec![], company: None, title: None, notes: None,
            source: "manual".into(),
        }).unwrap();

        save_contact(&store, &ck, &Contact {
            uid: "c2".into(), name: "Bob Smith".into(),
            first_name: Some("Bob".into()), last_name: Some("Smith".into()),
            emails: vec!["bob@example.com".into()],
            phones: vec![], company: None, title: None, notes: None,
            source: "manual".into(),
        }).unwrap();

        let results = resolve_contact(&store, &ck, "sarah").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Sarah Jones");

        cleanup(dir);
    }

    #[test]
    fn resolve_contact_by_email() {
        let (store, key, dir) = setup();
        let ck = contacts_key(&key);

        save_contact(&store, &ck, &Contact {
            uid: "c1".into(), name: "Alice".into(),
            first_name: None, last_name: None,
            emails: vec!["alice@wonderland.com".into()],
            phones: vec![], company: None, title: None, notes: None,
            source: "manual".into(),
        }).unwrap();

        let results = resolve_contact(&store, &ck, "wonderland").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Alice");

        cleanup(dir);
    }

    #[test]
    fn resolve_contact_by_company() {
        let (store, key, dir) = setup();
        let ck = contacts_key(&key);

        save_contact(&store, &ck, &Contact {
            uid: "c1".into(), name: "Team Lead".into(),
            first_name: None, last_name: None, emails: vec![],
            phones: vec![], company: Some("Acme Corp".into()),
            title: None, notes: None, source: "manual".into(),
        }).unwrap();

        let results = resolve_contact(&store, &ck, "acme").unwrap();
        assert_eq!(results.len(), 1);

        cleanup(dir);
    }

    #[test]
    fn resolve_contact_case_insensitive() {
        let (store, key, dir) = setup();
        let ck = contacts_key(&key);

        save_contact(&store, &ck, &Contact {
            uid: "c1".into(), name: "SARAH JONES".into(),
            first_name: Some("SARAH".into()), last_name: Some("JONES".into()),
            emails: vec![], phones: vec![], company: None,
            title: None, notes: None, source: "manual".into(),
        }).unwrap();

        let results = resolve_contact(&store, &ck, "sarah").unwrap();
        assert_eq!(results.len(), 1);

        cleanup(dir);
    }

    // ── Enrichment tests ─────────────────────────────────────────

    #[test]
    fn enrich_creates_new_contact() {
        let (store, key, dir) = setup();
        let ck = contacts_key(&key);

        let result = enrich_from_email_header(&store, &ck, "Jane Doe <jane@example.com>").unwrap();
        assert!(result.is_some());
        let contact = result.unwrap();
        assert_eq!(contact.name, "Jane Doe");
        assert_eq!(contact.emails, vec!["jane@example.com"]);
        assert_eq!(contact.first_name.as_deref(), Some("Jane"));
        assert_eq!(contact.last_name.as_deref(), Some("Doe"));
        assert_eq!(contact.source, "email-enrichment");

        // Should be in the store now
        let all = load_all_contacts(&store, &ck).unwrap();
        assert_eq!(all.len(), 1);

        cleanup(dir);
    }

    #[test]
    fn enrich_skips_existing_email() {
        let (store, key, dir) = setup();
        let ck = contacts_key(&key);

        // Pre-existing contact with this email
        save_contact(&store, &ck, &Contact {
            uid: "existing".into(), name: "Jane D.".into(),
            first_name: None, last_name: None,
            emails: vec!["jane@example.com".into()],
            phones: vec![], company: None, title: None, notes: None,
            source: "manual".into(),
        }).unwrap();

        let result = enrich_from_email_header(&store, &ck, "Jane Doe <jane@example.com>").unwrap();
        assert!(result.is_none(), "Should not create duplicate contact");

        let all = load_all_contacts(&store, &ck).unwrap();
        assert_eq!(all.len(), 1);

        cleanup(dir);
    }

    // ── Tool schema tests ────────────────────────────────────────

    #[test]
    fn search_contacts_schema() {
        let (store, key, dir) = setup();
        let tool = SearchContacts {
            store: store.clone(),
            key: contacts_key(&key),
        };
        assert_eq!(tool.name(), "search_contacts");
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "query"));
        cleanup(dir);
    }

    #[test]
    fn list_contacts_schema() {
        let (store, key, dir) = setup();
        let tool = ListContacts {
            store: store.clone(),
            key: contacts_key(&key),
        };
        assert_eq!(tool.name(), "list_contacts");
        cleanup(dir);
    }

    #[test]
    fn sync_contacts_schema() {
        let (store, key, dir) = setup();
        let config = ContactsConfig {
            url: "https://example.com".into(),
            username: "user".into(),
            password: "pass".into(),
            addressbook_path: None,
        };
        let tool = SyncContacts {
            config,
            store: store.clone(),
            key: contacts_key(&key),
        };
        assert_eq!(tool.name(), "sync_contacts");
        cleanup(dir);
    }

    #[test]
    fn addressbook_query_xml_valid() {
        let xml = build_addressbook_query_xml();
        assert!(xml.contains("addressbook-query"));
        assert!(xml.contains("address-data"));
    }

    #[test]
    fn unfold_continuation_lines() {
        let lines = unfold_vcard_lines("LINE1\r\n CONTINUED\r\nLINE2");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "LINE1CONTINUED");
        assert_eq!(lines[1], "LINE2");
    }

    // ── CUD tool tests ──────────────────────────────────────────

    #[test]
    fn add_contact_schema() {
        let (store, key, dir) = setup();
        let tool = AddContact { store, key: contacts_key(&key) };
        assert_eq!(tool.name(), "add_contact");
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "name"));
        cleanup(dir);
    }

    #[test]
    fn update_contact_schema() {
        let (store, key, dir) = setup();
        let tool = UpdateContact { store, key: contacts_key(&key) };
        assert_eq!(tool.name(), "update_contact");
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "uid"));
        cleanup(dir);
    }

    #[test]
    fn delete_contact_schema() {
        let (store, key, dir) = setup();
        let tool = DeleteContact { store, key: contacts_key(&key) };
        assert_eq!(tool.name(), "delete_contact");
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "uid"));
        cleanup(dir);
    }

    #[tokio::test]
    async fn add_update_delete_contact_round_trip() {
        let (store, key, dir) = setup();
        let ck = contacts_key(&key);

        // Add
        let add = AddContact { store: store.clone(), key: aivyx_crypto::MasterKey::from_bytes(ck.expose_secret().try_into().unwrap()) };
        let result = add.execute(serde_json::json!({
            "name": "Test User",
            "emails": ["test@example.com"],
        })).await.unwrap();
        let uid = result["uid"].as_str().unwrap().to_string();
        assert_eq!(result["status"], "created");

        // Verify exists
        let all = load_all_contacts(&store, &ck).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Test User");

        // Update
        let update = UpdateContact { store: store.clone(), key: aivyx_crypto::MasterKey::from_bytes(ck.expose_secret().try_into().unwrap()) };
        let result = update.execute(serde_json::json!({
            "uid": uid,
            "name": "Updated User",
            "company": "TestCorp",
        })).await.unwrap();
        assert_eq!(result["status"], "updated");

        let all = load_all_contacts(&store, &ck).unwrap();
        assert_eq!(all[0].name, "Updated User");
        assert_eq!(all[0].company.as_deref(), Some("TestCorp"));

        // Delete
        let delete = DeleteContact { store: store.clone(), key: aivyx_crypto::MasterKey::from_bytes(ck.expose_secret().try_into().unwrap()) };
        let result = delete.execute(serde_json::json!({ "uid": uid })).await.unwrap();
        assert_eq!(result["status"], "deleted");

        let all = load_all_contacts(&store, &ck).unwrap();
        assert!(all.is_empty());

        cleanup(dir);
    }
}
