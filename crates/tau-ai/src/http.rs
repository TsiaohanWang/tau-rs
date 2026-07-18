//! HTTP client helpers with proxy normalization.
//!
//! Mirrors `tau_ai.http`: reqwest client construction with SOCKS proxy
//! scheme normalization and shared client configuration.

use std::time::Duration;

/// Normalize SOCKS proxy URL schemes for reqwest compatibility.
///
/// Converts `socks://` → `socks5://` (reqwest doesn't accept bare `socks://`).
pub fn normalize_proxy_url(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("socks://") {
        format!("socks5://{rest}")
    } else {
        url.to_string()
    }
}

/// Configuration for building an HTTP client.
#[derive(Debug, Clone)]
pub struct HttpClientConfig {
    pub timeout: Duration,
    pub proxy_url: Option<String>,
    pub user_agent: Option<String>,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        HttpClientConfig {
            timeout: Duration::from_secs(300),
            proxy_url: None,
            user_agent: Some("tau-rs/0.1".to_string()),
        }
    }
}

/// Build a reqwest client with the given configuration.
pub fn build_client(config: &HttpClientConfig) -> Result<reqwest::Client, reqwest::Error> {
    let mut builder = reqwest::Client::builder().timeout(config.timeout);

    if let Some(ref ua) = config.user_agent {
        builder = builder.user_agent(ua);
    }

    if let Some(ref proxy) = config.proxy_url {
        let normalized = normalize_proxy_url(proxy);
        builder = builder.proxy(reqwest::Proxy::all(&normalized)?);
    }

    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_socks_proxy() {
        assert_eq!(
            normalize_proxy_url("socks://localhost:1080"),
            "socks5://localhost:1080"
        );
    }

    #[test]
    fn normalize_socks5_passthrough() {
        assert_eq!(
            normalize_proxy_url("socks5://localhost:1080"),
            "socks5://localhost:1080"
        );
    }

    #[test]
    fn normalize_http_proxy() {
        assert_eq!(
            normalize_proxy_url("http://proxy:8080"),
            "http://proxy:8080"
        );
    }
}
