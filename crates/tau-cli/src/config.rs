use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::debug;

/// Top-level configuration directory (~/.tau).
#[derive(Debug, Clone)]
pub struct TauHome {
    pub root: PathBuf,
}

impl TauHome {
    pub fn discover() -> Self {
        let root = if let Ok(val) = std::env::var("TAU_HOME") {
            PathBuf::from(val)
        } else {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".tau")
        };
        TauHome { root }
    }

    pub fn providers_path(&self) -> PathBuf {
        self.root.join("providers.json")
    }

    pub fn credentials_path(&self) -> PathBuf {
        self.root.join("credentials.json")
    }

    pub fn catalog_path(&self) -> PathBuf {
        self.root.join("catalog.toml")
    }
}

// ---------------------------------------------------------------------------
// providers.json
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct ProvidersConfig {
    #[serde(default = "default_provider")]
    pub default_provider: String,
    #[serde(default)]
    pub provider_preferences: HashMap<String, ProviderPrefs>,
}

fn default_provider() -> String {
    "openai".to_string()
}

#[derive(Debug, Clone, Deserialize, Default)]
#[allow(dead_code)]
pub struct ProviderPrefs {
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default)]
    pub max_retries: Option<u32>,
    #[serde(default)]
    pub max_retry_delay_seconds: Option<f64>,
    #[serde(default)]
    pub thinking_defaults: Option<serde_json::Value>,
    #[serde(default)]
    pub timeout_seconds: Option<f64>,
}

impl ProvidersConfig {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            debug!("no providers.json found, using defaults");
            return Ok(Self {
                default_provider: default_provider(),
                provider_preferences: HashMap::new(),
            });
        }
        let text = std::fs::read_to_string(path)?;
        let config: Self = serde_json::from_str(&text)?;
        debug!(default = %config.default_provider, providers = config.provider_preferences.len(), "loaded providers.json");
        Ok(config)
    }
}

// ---------------------------------------------------------------------------
// credentials.json
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Default)]
pub struct CredentialsConfig(HashMap<String, String>);

impl CredentialsConfig {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            debug!("no credentials.json found");
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)?;
        let config: Self = serde_json::from_str(&text)?;
        debug!(keys = config.0.len(), "loaded credentials.json");
        Ok(config)
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(|s| s.as_str())
    }
}

// ---------------------------------------------------------------------------
// catalog.toml
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct CatalogConfig {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub providers: Vec<CatalogProvider>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct CatalogProvider {
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub credential_name: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub docs_url: Option<String>,
    #[serde(default)]
    pub thinking_levels: Option<Vec<String>>,
    #[serde(default)]
    pub thinking_models: Option<Vec<String>>,
    #[serde(default)]
    pub thinking_default: Option<String>,
    #[serde(default)]
    pub thinking_parameter: Option<String>,
    #[serde(default)]
    pub context_windows: Option<HashMap<String, u64>>,
}

impl CatalogConfig {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            debug!("no catalog.toml found");
            return Ok(CatalogConfig {
                schema_version: 1,
                providers: vec![],
            });
        }
        let text = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&text)?;
        debug!(providers = config.providers.len(), "loaded catalog.toml");
        Ok(config)
    }

    pub fn find_provider(&self, name: &str) -> Option<&CatalogProvider> {
        self.providers.iter().find(|p| p.name == name)
    }
}

// ---------------------------------------------------------------------------
// Credential resolution
// ---------------------------------------------------------------------------

/// Resolve the API key for a given provider using:
/// 1. Environment variable (from catalog `api_key_env`)
/// 2. Credentials file (from catalog `credential_name`)
pub fn resolve_api_key(
    catalog: &CatalogConfig,
    credentials: &CredentialsConfig,
    provider_name: &str,
) -> Option<String> {
    if let Some(cp) = catalog.find_provider(provider_name) {
        // 1. Environment variable
        if let Some(env_name) = &cp.api_key_env {
            if let Ok(val) = std::env::var(env_name) {
                if !val.is_empty() {
                    debug!(
                        provider = provider_name,
                        env = env_name,
                        "resolved API key from env"
                    );
                    return Some(val);
                }
            }
        }
        // 2. Credentials file
        if let Some(cred_name) = &cp.credential_name {
            if let Some(key) = credentials.get(cred_name) {
                debug!(
                    provider = provider_name,
                    cred = cred_name,
                    "resolved API key from credentials file"
                );
                return Some(key.to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Provider kind detection
// ---------------------------------------------------------------------------

/// Determine if a provider should use the Anthropic or OpenAI wire protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Anthropic,
    OpenaiCompatible,
}

impl ProviderKind {
    pub fn from_catalog(catalog: &CatalogConfig, provider_name: &str) -> Self {
        if let Some(cp) = catalog.find_provider(provider_name) {
            match cp.kind.as_deref() {
                Some("anthropic") => ProviderKind::Anthropic,
                _ => ProviderKind::OpenaiCompatible,
            }
        } else {
            // Fallback: well-known names
            match provider_name {
                "anthropic" => ProviderKind::Anthropic,
                _ => ProviderKind::OpenaiCompatible,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn tmp_dir() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tau-cli-test-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn load_providers_json() {
        let dir = tmp_dir();
        let path = dir.join("providers.json");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"default_provider":"anthropic","provider_preferences":{{"anthropic":{{"default_model":"claude-sonnet"}}}}}}"#
        )
        .unwrap();

        let cfg = ProvidersConfig::load(&path).unwrap();
        assert_eq!(cfg.default_provider, "anthropic");
        let prefs = cfg.provider_preferences.get("anthropic").unwrap();
        assert_eq!(prefs.default_model.as_deref(), Some("claude-sonnet"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_catalog_toml() {
        let dir = tmp_dir();
        let path = dir.join("catalog.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"schema_version = 1

[[providers]]
name = "deepseek"
kind = "openai-compatible"
base_url = "https://api.deepseek.com"
api_key_env = "DEEPSEEK_API_KEY"
credential_name = "deepseek"
models = ["deepseek-v4-flash"]
default_model = "deepseek-v4-flash"
"#
        )
        .unwrap();

        let cfg = CatalogConfig::load(&path).unwrap();
        assert_eq!(cfg.schema_version, 1);
        assert_eq!(cfg.providers.len(), 1);
        let p = &cfg.providers[0];
        assert_eq!(p.name, "deepseek");
        assert_eq!(p.base_url.as_deref(), Some("https://api.deepseek.com"));
        assert_eq!(p.api_key_env.as_deref(), Some("DEEPSEEK_API_KEY"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_api_key_from_env() {
        let dir = tmp_dir();
        let cat_path = dir.join("catalog.toml");
        let mut f = std::fs::File::create(&cat_path).unwrap();
        writeln!(
            f,
            r#"[[providers]]
name = "test"
api_key_env = "TAU_TEST_KEY"
"#
        )
        .unwrap();
        let catalog = CatalogConfig::load(&cat_path).unwrap();
        let creds = CredentialsConfig::default();

        // SAFETY: test runs single-threaded by default
        unsafe {
            std::env::set_var("TAU_TEST_KEY", "test-secret-123");
        }
        let key = resolve_api_key(&catalog, &creds, "test");
        assert_eq!(key.as_deref(), Some("test-secret-123"));
        unsafe {
            std::env::remove_var("TAU_TEST_KEY");
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_api_key_from_credentials_file() {
        let dir = tmp_dir();
        let cat_path = dir.join("catalog.toml");
        let mut f = std::fs::File::create(&cat_path).unwrap();
        writeln!(
            f,
            r#"[[providers]]
name = "test"
credential_name = "mycred"
"#
        )
        .unwrap();
        let catalog = CatalogConfig::load(&cat_path).unwrap();

        let cred_path = dir.join("credentials.json");
        let mut f2 = std::fs::File::create(&cred_path).unwrap();
        writeln!(f2, r#"{{"mycred": "cred-file-key"}}"#).unwrap();
        let creds = CredentialsConfig::load(&cred_path).unwrap();

        let key = resolve_api_key(&catalog, &creds, "test");
        assert_eq!(key.as_deref(), Some("cred-file-key"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
