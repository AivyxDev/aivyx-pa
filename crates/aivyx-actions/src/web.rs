//! Web actions — fetch pages and search the internet.

use crate::Action;
use aivyx_core::Result;

pub struct FetchPage;

#[async_trait::async_trait]
impl Action for FetchPage {
    fn name(&self) -> &str {
        "fetch_webpage"
    }

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
        let url = input["url"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("url is required".into()))?
            .to_string();

        validate_url(&url)?;

        let (status, body) = crate::retry::retry(
            &crate::retry::RetryConfig::network(),
            || async {
                let client = crate::http_client();
                let response = client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| aivyx_core::AivyxError::Http(e.to_string()))?;
                let status = response.status().as_u16();
                let body = response
                    .text()
                    .await
                    .map_err(|e| aivyx_core::AivyxError::Http(e.to_string()))?;
                Ok((status, body))
            },
            crate::retry::is_transient,
        )
        .await?;

        // Truncate to avoid blowing up context (char-boundary safe)
        let truncated = truncate_safe(&body, 32_000);

        Ok(serde_json::json!({
            "url": url,
            "status": status,
            "content": truncated,
        }))
    }
}

/// Validate a URL for safety (SSRF prevention).
///
/// Rejects non-HTTP schemes, localhost, private IPs, and link-local addresses.
fn validate_url(url: &str) -> Result<()> {
    // Must start with http:// or https://
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(aivyx_core::AivyxError::Validation(
            "Only http:// and https:// URLs are allowed".into(),
        ));
    }

    // Extract host portion (between :// and next / or end)
    let after_scheme = url.split("://").nth(1).unwrap_or("");
    let host = after_scheme
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_lowercase();

    // Block localhost
    if host == "localhost"
        || host == "127.0.0.1"
        || host == "::1"
        || host == "[::1]"
        || host == "0.0.0.0"
    {
        return Err(aivyx_core::AivyxError::Validation(
            "Localhost URLs are not allowed".into(),
        ));
    }

    // Block private/link-local IPv4 ranges
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>()
        && (ip.is_private()
            || ip.is_loopback()
            || ip.is_link_local()
            || ip.is_broadcast()
            || host.starts_with("169.254."))
    // link-local / cloud metadata
    {
        return Err(aivyx_core::AivyxError::Validation(format!(
            "Private/internal IP addresses are not allowed: {host}"
        )));
    }

    // Block private/link-local IPv6 ranges (strip brackets for [::1] notation)
    let ipv6_host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = ipv6_host.parse::<std::net::Ipv6Addr>() {
        if ip.is_loopback() || ip.is_unspecified()
            || is_ipv6_unique_local(&ip)   // fc00::/7  — ULA (private)
            || is_ipv6_link_local(&ip)
        // fe80::/10 — link-local
        {
            return Err(aivyx_core::AivyxError::Validation(format!(
                "Private/internal IPv6 addresses are not allowed: {host}"
            )));
        }
        // Check for IPv4-mapped IPv6 (::ffff:127.0.0.1) — common SSRF bypass
        if let Some(ipv4) = ip.to_ipv4_mapped()
            && (ipv4.is_private() || ipv4.is_loopback() || ipv4.is_link_local())
        {
            return Err(aivyx_core::AivyxError::Validation(format!(
                "IPv4-mapped IPv6 to private address not allowed: {host}"
            )));
        }
    }

    // Block common cloud metadata endpoints
    if host == "metadata.google.internal" || host.ends_with(".internal") {
        return Err(aivyx_core::AivyxError::Validation(
            "Internal/metadata endpoints are not allowed".into(),
        ));
    }

    Ok(())
}

/// Check if an IPv6 address is in the Unique Local Address range (fc00::/7).
/// Equivalent to the unstable `Ipv6Addr::is_unique_local()`.
fn is_ipv6_unique_local(ip: &std::net::Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

/// Check if an IPv6 address is link-local (fe80::/10).
/// Equivalent to the unstable `Ipv6Addr::is_unicast_link_local()`.
fn is_ipv6_link_local(ip: &std::net::Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

/// Truncate a string at a safe UTF-8 char boundary.
fn truncate_safe(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        s.to_string()
    } else {
        // Walk back to find a valid char boundary
        let mut end = max_bytes;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...[truncated]", &s[..end])
    }
}

/// Search the web using DuckDuckGo's HTML lite interface.
///
/// Returns up to `max_results` search results with titles, URLs, and snippets.
/// Uses the HTML lite endpoint which requires no API key and returns
/// structured results that can be parsed without JavaScript rendering.
pub struct SearchWeb;

#[async_trait::async_trait]
impl Action for SearchWeb {
    fn name(&self) -> &str {
        "search_web"
    }

    fn description(&self) -> &str {
        "Search the web and return results with titles, URLs, and snippets"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum results to return (default: 5, max: 10)",
                    "default": 5
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let query = input["query"].as_str().unwrap_or_default();
        let max_results = input["max_results"].as_u64().unwrap_or(5).min(10) as usize;

        if query.is_empty() {
            return Err(aivyx_core::AivyxError::Other(
                "query must not be empty".into(),
            ));
        }

        let query_owned = query.to_string();
        let results = crate::retry::retry(
            &crate::retry::RetryConfig::network(),
            || search_ddg(&query_owned, max_results),
            crate::retry::is_transient,
        )
        .await?;

        Ok(serde_json::json!({
            "query": query_owned,
            "results": results,
            "count": results.len(),
        }))
    }
}

/// Perform a search via DuckDuckGo HTML lite and parse results.
async fn search_ddg(query: &str, max_results: usize) -> Result<Vec<serde_json::Value>> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (compatible; Aivyx/1.0)")
        .build()
        .map_err(|e| aivyx_core::AivyxError::Http(e.to_string()))?;

    let resp = client
        .get("https://html.duckduckgo.com/html/")
        .query(&[("q", query)])
        .send()
        .await
        .map_err(|e| aivyx_core::AivyxError::Http(e.to_string()))?;

    let body = resp
        .text()
        .await
        .map_err(|e| aivyx_core::AivyxError::Http(e.to_string()))?;

    Ok(parse_ddg_html(&body, max_results))
}

/// Parse DuckDuckGo HTML search results.
///
/// The HTML lite page uses a predictable structure:
/// - Each result is in a `<div class="result">` or `<div class="web-result">`
/// - Title is in `<a class="result__a">` with the URL as href
/// - Snippet is in `<a class="result__snippet">`
///
/// We use simple string scanning rather than a full HTML parser to avoid
/// adding a dependency for this single use case.
fn parse_ddg_html(html: &str, max_results: usize) -> Vec<serde_json::Value> {
    let mut results = Vec::new();

    // Split on result link anchors — each result__a marks a new search result
    for chunk in html.split("class=\"result__a\"") {
        if results.len() >= max_results {
            break;
        }

        // Extract href from the anchor
        let url = extract_between(chunk, "href=\"", "\"").map(|u| {
            // DDG wraps URLs in a redirect; extract the actual URL
            if let Some(pos) = u.find("uddg=") {
                url_decode(&u[pos + 5..])
            } else {
                u.to_string()
            }
        });

        // Extract title text (content between > and </a>)
        let title = extract_between(chunk, ">", "</a>").map(strip_html_tags);

        // Look for snippet
        let snippet = if let Some(snippet_start) = chunk.find("class=\"result__snippet\"") {
            let rest = &chunk[snippet_start..];
            extract_between(rest, ">", "</a>")
                .or_else(|| extract_between(rest, ">", "</"))
                .map(strip_html_tags)
        } else {
            None
        };

        if let (Some(url), Some(title)) = (url, title)
            && !url.is_empty()
            && !title.is_empty()
        {
            results.push(serde_json::json!({
                "title": title.trim(),
                "url": url.trim(),
                "snippet": snippet.unwrap_or_default().trim(),
            }));
        }
    }

    results
}

/// Extract text between two delimiters.
fn extract_between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let start_pos = s.find(start)? + start.len();
    let rest = &s[start_pos..];
    let end_pos = rest.find(end)?;
    Some(&rest[..end_pos])
}

/// Minimal URL decoding for DDG redirect URLs.
///
/// Collects percent-decoded bytes into a buffer first, then converts
/// to a UTF-8 string. This correctly handles multi-byte sequences
/// like `%C3%A9` (é).
fn url_decode(s: &str) -> String {
    let mut bytes = Vec::with_capacity(s.len());
    let mut chars = s.as_bytes().iter().copied();
    while let Some(b) = chars.next() {
        match b {
            b'%' => {
                let hex: Vec<u8> = chars.by_ref().take(2).collect();
                if hex.len() == 2 {
                    if let Ok(decoded) = u8::from_str_radix(&String::from_utf8_lossy(&hex), 16) {
                        bytes.push(decoded);
                    } else {
                        bytes.push(b'%');
                        bytes.extend_from_slice(&hex);
                    }
                } else {
                    bytes.push(b'%');
                    bytes.extend_from_slice(&hex);
                }
            }
            b'+' => bytes.push(b' '),
            b'&' => break, // Stop at next query parameter
            _ => bytes.push(b),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Strip HTML tags from a string, keeping only text content.
fn strip_html_tags(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }
    // Decode common HTML entities
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_between_basic() {
        assert_eq!(
            extract_between("href=\"https://example.com\" class", "href=\"", "\""),
            Some("https://example.com")
        );
    }

    #[test]
    fn extract_between_missing() {
        assert_eq!(extract_between("no match", "href=\"", "\""), None);
    }

    #[test]
    fn url_decode_basic() {
        assert_eq!(url_decode("hello%20world"), "hello world");
        assert_eq!(url_decode("a%2Fb"), "a/b");
    }

    #[test]
    fn url_decode_stops_at_ampersand() {
        assert_eq!(
            url_decode("https://example.com&rut=abc"),
            "https://example.com"
        );
    }

    #[test]
    fn strip_html_basic() {
        assert_eq!(strip_html_tags("<b>bold</b> text"), "bold text");
        assert_eq!(strip_html_tags("no tags"), "no tags");
    }

    #[test]
    fn strip_html_entities() {
        assert_eq!(strip_html_tags("a &amp; b"), "a & b");
        assert_eq!(strip_html_tags("&lt;tag&gt;"), "<tag>");
    }

    #[test]
    fn parse_ddg_empty() {
        assert!(parse_ddg_html("<html>no results</html>", 5).is_empty());
    }

    #[test]
    fn parse_ddg_synthetic_result() {
        let html = r#"
        <div>
        class="result__a" href="https://example.com">Example Site</a>
        class="result__snippet">This is a test snippet.</a>
        </div>
        "#;
        let results = parse_ddg_html(html, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["title"], "Example Site");
        assert_eq!(results[0]["url"], "https://example.com");
        assert_eq!(results[0]["snippet"], "This is a test snippet.");
    }

    #[test]
    fn parse_ddg_respects_max_results() {
        let html = r#"
        class="result__a" href="https://a.com">A</a>
        class="result__a" href="https://b.com">B</a>
        class="result__a" href="https://c.com">C</a>
        "#;
        let results = parse_ddg_html(html, 2);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_web_schema_requires_query() {
        let tool = SearchWeb;
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "query"));
    }

    #[test]
    fn fetch_page_schema_requires_url() {
        let tool = FetchPage;
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "url"));
    }

    // SSRF protection tests
    #[test]
    fn validate_url_allows_https() {
        assert!(validate_url("https://example.com/page").is_ok());
        assert!(validate_url("http://example.com").is_ok());
    }

    #[test]
    fn validate_url_rejects_non_http() {
        assert!(validate_url("file:///etc/passwd").is_err());
        assert!(validate_url("ftp://example.com").is_err());
        assert!(validate_url("gopher://evil.com").is_err());
    }

    #[test]
    fn validate_url_rejects_localhost() {
        assert!(validate_url("http://localhost/secret").is_err());
        assert!(validate_url("http://127.0.0.1/admin").is_err());
        assert!(validate_url("http://0.0.0.0/").is_err());
    }

    #[test]
    fn validate_url_rejects_private_ips() {
        assert!(validate_url("http://10.0.0.1/internal").is_err());
        assert!(validate_url("http://192.168.1.1/admin").is_err());
        assert!(validate_url("http://172.16.0.1/secret").is_err());
    }

    #[test]
    fn validate_url_rejects_cloud_metadata() {
        assert!(validate_url("http://169.254.169.254/latest/meta-data/").is_err());
        assert!(validate_url("http://metadata.google.internal/computeMetadata/v1/").is_err());
    }

    // Truncation tests
    #[test]
    fn truncate_safe_within_limit() {
        assert_eq!(truncate_safe("hello", 10), "hello");
    }

    #[test]
    fn truncate_safe_at_limit() {
        let result = truncate_safe("hello world", 5);
        assert!(result.starts_with("hello"));
        assert!(result.contains("[truncated]"));
    }

    #[test]
    fn truncate_safe_multibyte() {
        // é is 2 bytes in UTF-8; cutting at byte 1 would panic with &s[..1]
        let s = "é".repeat(100); // 200 bytes
        let result = truncate_safe(&s, 5);
        // Should not panic, and should end at a char boundary
        assert!(result.contains("[truncated]"));
    }

    // url_decode multi-byte test
    #[test]
    fn url_decode_multibyte_utf8() {
        // %C3%A9 is é in UTF-8
        assert_eq!(url_decode("%C3%A9"), "é");
    }
}
