// src/proxy/url_mapper.rs
//! URL Mapper
//! ==========
//!
//! Converts proxy URLs to real HTTPS URLs

use anyhow::{Result, Context};

/// Maps proxy HTTP URLs to real HTTPS URLs
///
/// # Examples
///
/// ```rust
/// use yandi::proxy::UrlMapper;
///
/// // Browser: http://localhost:8080/youtube.com/watch?v=xxx
/// // Maps to: https://youtube.com/watch?v=xxx
/// ```
pub struct UrlMapper;

impl UrlMapper {
    /// Convert proxy URL to real URL (preserves original scheme)
    ///
    /// # Input formats
    ///
    /// 1. Absolute URL:
    ///    - `http://localhost:8080/youtube.com/watch?v=xxx`
    ///    - `http://127.0.0.1:8080/twitter.com/user`
    ///
    /// 2. Path-only format (after proxy header):
    ///    - `youtube.com/watch?v=xxx` (defaults to https://)
    ///    - `twitter.com/user/status/123` (defaults to https://)
    ///
    /// 3. Absolute URL with target scheme:
    ///    - `http://goodwin.su/` (keeps http://)
    ///    - `https://google.com/` (keeps https://)
    ///
    /// # Output
    ///
    /// Returns URL with proper scheme:
    /// - `https://youtube.com/watch?v=xxx` (path-only → https)
    /// - `http://goodwin.su/` (preserves original scheme)
    /// - `https://google.com/` (preserves original scheme)
    pub fn proxy_to_real(proxy_url: &str) -> Result<String> {
        // Remove proxy prefix if present
        let path = Self::extract_path(proxy_url)?;

        // Check if already has protocol (absolute URL from browser)
        if path.starts_with("http://") || path.starts_with("https://") {
            // Keep original scheme!
            return Ok(path);
        }

        // Parse domain and path
        // Format: "youtube.com/watch?v=xxx", "youtube.com", or "/youtube.com/watch?v=xxx"
        let path = path.trim_start_matches('/');
        let parts: Vec<&str> = path.splitn(2, '/').collect();
        let domain = parts.first().context("No domain found in URL")?;

        // Validate domain (basic check)
        if !domain.contains('.') {
            return Err(anyhow::anyhow!("Invalid domain: {}", domain));
        }

        // Build URL (default to HTTPS for path-only format)
        let rest = if parts.len() > 1 { parts[1] } else { "" };

        if rest.is_empty() {
            Ok(format!("https://{}", domain))
        } else {
            Ok(format!("https://{}/{}", domain, rest))
        }
    }

    /// Extract path from proxy URL
    ///
    /// # Examples
    ///
    /// - `http://localhost:8080/youtube.com/watch` → `youtube.com/watch`
    /// - `youtube.com/watch` → `youtube.com/watch`
    /// - `http://goodwin.su/` → `http://goodwin.su/` (absolute URL - keep as-is!)
    fn extract_path(proxy_url: &str) -> Result<String> {
        let url = proxy_url.trim();

        // If no protocol, return as-is
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Ok(url.to_string());
        }

        // Check if this is our proxy URL (localhost:8080 or 127.0.0.1:8080)
        if let Some(start) = url.find("://") {
            let after_proto = &url[start + 3..];

            // Check if it's our proxy (localhost:8080 or 127.0.0.1:8080)
            if after_proto.starts_with("localhost:8080/") ||
               after_proto.starts_with("127.0.0.1:8080/") {
                // This is our proxy URL - extract the path after proxy
                if let Some(slash_pos) = after_proto.find('/') {
                    let path = &after_proto[slash_pos + 1..];
                    if !path.is_empty() {
                        return Ok(path.to_string());
                    }
                }
                return Err(anyhow::anyhow!("No path in proxy URL"));
            }

            // Otherwise - it's an absolute URL from browser (http://goodwin.su/)
            // Keep it as-is!
            return Ok(url.to_string());
        }

        Err(anyhow::anyhow!("Invalid proxy URL format"))
    }

    /// Parse HTTP request line to extract target
    ///
    /// # Input
    ///
    /// HTTP request line like:
    /// - `GET /youtube.com/watch?v=xxx HTTP/1.1`
    /// - `GET http://localhost:8080/youtube.com/watch?v=xxx HTTP/1.1`
    /// - `CONNECT muntyan-photonics.su:443 HTTP/1.1`
    ///
    /// # Output
    ///
    /// Real HTTPS URL:
    /// - `https://youtube.com/watch?v=xxx`
    /// - `goodwin.su:443` (for CONNECT - NO https:// prefix!)
    pub fn parse_request_line(line: &str) -> Result<(String, String)> {
        let parts: Vec<&str> = line.split_whitespace().collect();

        if parts.len() < 2 {
            return Err(anyhow::anyhow!("Invalid HTTP request line"));
        }

        let method = parts[0];
        let url = parts[1];

        // Handle CONNECT method (for HTTPS tunneling)
        if method == "CONNECT" {
            // CONNECT format: host:port
            // Example: CONNECT goodwin.su:443 HTTP/1.1
            // IMPORTANT: Return as-is WITHOUT https:// prefix!
            // Gateway will return "200 Connection Established"
            return Ok((method.to_string(), url.to_string()));
        }

        // Only support GET, POST, HEAD
        if method != "GET" && method != "POST" && method != "HEAD" {
            return Err(anyhow::anyhow!("Unsupported HTTP method: {}", method));
        }

        // Convert proxy URL to real URL
        let real_url = Self::proxy_to_real(url)?;
        Ok((method.to_string(), real_url))
    }

    /// Extract domain from URL
    ///
    /// # Examples
    ///
    /// - `https://youtube.com/watch?v=xxx` → `youtube.com`
    /// - `https://twitter.com:443/user` → `twitter.com`
    pub fn extract_domain(url: &str) -> Result<String> {
        let url = Self::proxy_to_real(url)?;

        if let Some(start) = url.find("://") {
            let after_proto = &url[start + 3..];

            // Remove port and path
            let domain = after_proto
                .split('/')
                .next()
                .context("No domain in URL")?
                .split(':')
                .next()
                .context("No domain in URL")?;

            Ok(domain.to_string())
        } else {
            Err(anyhow::anyhow!("Invalid URL format"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_to_real_path_format() {
        // Path-only format defaults to https://
        let url = UrlMapper::proxy_to_real("youtube.com/watch?v=xxx").unwrap();
        assert_eq!(url, "https://youtube.com/watch?v=xxx");
    }

    #[test]
    fn test_proxy_to_real_full_url() {
        // Full proxy URL → extracted path → defaults to https://
        let url = UrlMapper::proxy_to_real("http://localhost:8080/youtube.com/watch?v=xxx").unwrap();
        assert_eq!(url, "https://youtube.com/watch?v=xxx");
    }

    #[test]
    fn test_proxy_to_real_preserves_http_scheme() {
        // Absolute URL with http:// scheme should be preserved
        let url = UrlMapper::proxy_to_real("http://goodwin.su/").unwrap();
        assert_eq!(url, "http://goodwin.su/");
    }

    #[test]
    fn test_proxy_to_real_preserves_https_scheme() {
        // Absolute URL with https:// scheme should be preserved
        let url = UrlMapper::proxy_to_real("https://google.com/").unwrap();
        assert_eq!(url, "https://google.com/");
    }

    #[test]
    fn test_proxy_to_real_domain_only() {
        let url = UrlMapper::proxy_to_real("youtube.com").unwrap();
        assert_eq!(url, "https://youtube.com");
    }

    #[test]
    fn test_extract_domain() {
        let domain = UrlMapper::extract_domain("youtube.com/watch?v=xxx").unwrap();
        assert_eq!(domain, "youtube.com");
    }

    #[test]
    fn test_parse_request_line_get() {
        let line = "GET /youtube.com/watch?v=xxx HTTP/1.1";
        let (method, url) = UrlMapper::parse_request_line(line).unwrap();
        assert_eq!(method, "GET");
        assert_eq!(url, "https://youtube.com/watch?v=xxx");
    }

    #[test]
    fn test_parse_request_line_connect() {
        let line = "CONNECT goodwin.su:443 HTTP/1.1";
        let (method, url) = UrlMapper::parse_request_line(line).unwrap();
        assert_eq!(method, "CONNECT");
        // CONNECT should NOT have https:// prefix!
        assert_eq!(url, "goodwin.su:443");
    }
}
