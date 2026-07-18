//! Built-in and user catalog configuration.
//!
//! Mirrors the `[[providers]]` rows of `catalog.toml`. The base catalog is
//! embedded via `include_str!` from `../../data/catalog.toml`; the user may
//! override providers or add new ones in `~/.tau/catalog.toml`. The merge is a
//! shallow-priority list where overlay providers replace same-named base
//! entries and unique entries on either side are preserved.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Catalog root: a `schema_version` plus a flat list of provider rows.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CatalogConfig {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub providers: Vec<CatalogProvider>,
}

/// One provider row in the catalog.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CatalogProvider {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_name: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_levels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_models: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_parameter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_windows: Option<HashMap<String, u64>>,
}

/// A coarse enum for translating catalog `kind` / `api` into a provider branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Anthropic,
    OpenaiCompatible,
    OpenaiResponses,
}

impl ProviderKind {
    pub fn from_provider(p: &CatalogProvider) -> Self {
        let k = p.kind.as_deref().unwrap_or("");
        let api = p.api.as_deref().unwrap_or("");
        match (k, api) {
            (_, "openai-responses") => Self::OpenaiResponses,
            ("anthropic", _) => Self::Anthropic,
            _ => Self::OpenaiCompatible,
        }
    }
}

const BUILTIN_CATALOG: &str = include_str!("../../data/catalog.toml");

pub fn builtin() -> CatalogConfig {
    toml::from_str(BUILTIN_CATALOG).expect("built-in catalog.toml must parse")
}

/// Deep-merge two catalogs.
///
/// Rules:
/// - **Providers**: overlay providers replace same-named base provider; new
///   providers on either side are preserved in order (base first, then
///   overlay-only providers, matching the Python catalog loader).
/// - **schema_version**: overlay wins when non-zero, else base.
pub fn merge_catalogs(base: &CatalogConfig, overlay: &CatalogConfig) -> CatalogConfig {
    let mut by_name: HashMap<String, CatalogProvider> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    for p in &base.providers {
        if !by_name.contains_key(&p.name) {
            order.push(p.name.clone());
        }
        by_name.insert(p.name.clone(), p.clone());
    }
    for p in &overlay.providers {
        match by_name.get(&p.name) {
            // Replace same-named provider entirely (Python deep-merge mutates
            // individual fields; the base is a template so a wholesale replace is
            // the user-visible behavior of "I want my entry to look like X").
            Some(_) => {
                by_name.insert(p.name.clone(), p.clone());
            }
            None => {
                order.push(p.name.clone());
                by_name.insert(p.name.clone(), p.clone());
            }
        }
    }

    let providers = order
        .into_iter()
        .map(|name| by_name.remove(&name).expect("present in order"))
        .collect();

    let schema_version = if overlay.schema_version != 0 {
        overlay.schema_version
    } else {
        base.schema_version
    };

    CatalogConfig {
        schema_version,
        providers,
    }
}

pub fn load_user_or_default(path: &std::path::Path) -> anyhow::Result<CatalogConfig> {
    let user = if path.exists() {
        let text = std::fs::read_to_string(path)?;
        toml::from_str::<CatalogConfig>(&text)?
    } else {
        CatalogConfig::default()
    };
    Ok(merge_catalogs(&builtin(), &user))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(name: &str, base_url: &str) -> CatalogProvider {
        CatalogProvider {
            name: name.into(),
            base_url: Some(base_url.into()),
            ..Default::default()
        }
    }

    #[test]
    fn builtin_loads_and_has_providers() {
        let c = builtin();
        assert!(c.schema_version >= 1);
        assert!(c.providers.iter().any(|p| p.name == "openai"));
        // Sanity-check a couple of well-known providers exist.
        let names: Vec<&str> = c.providers.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"anthropic"));
        assert!(names.contains(&"openai"));
    }

    #[test]
    fn merge_with_empty_overlay_returns_base() {
        let base = builtin();
        let merged = merge_catalogs(&base, &CatalogConfig::default());
        assert_eq!(merged.providers.len(), base.providers.len());
        assert_eq!(merged.schema_version, base.schema_version);
    }

    #[test]
    fn merge_overlay_replaces_same_named_provider() {
        let base = CatalogConfig {
            schema_version: 1,
            providers: vec![provider("openai", "https://base.example")],
        };
        let overlay = CatalogConfig {
            schema_version: 2,
            providers: vec![provider("openai", "https://user.example")],
        };
        let merged = merge_catalogs(&base, &overlay);
        assert_eq!(merged.providers.len(), 1);
        assert_eq!(
            merged.providers[0].base_url.as_deref(),
            Some("https://user.example")
        );
        assert_eq!(merged.schema_version, 2);
    }

    #[test]
    fn merge_appends_overlay_only_provider() {
        let base = CatalogConfig {
            schema_version: 1,
            providers: vec![provider("openai", "https://a")],
        };
        let overlay = CatalogConfig {
            schema_version: 0,
            providers: vec![provider("myllm", "https://my")],
        };
        let merged = merge_catalogs(&base, &overlay);
        assert_eq!(merged.providers.len(), 2);
        assert_eq!(merged.providers[0].name, "openai");
        assert_eq!(merged.providers[1].name, "myllm");
        // schema_version falls back to base when overlay is 0.
        assert_eq!(merged.schema_version, 1);
    }

    #[test]
    fn merge_preserves_base_order_and_dedups_overlay() {
        let base = CatalogConfig {
            schema_version: 1,
            providers: vec![provider("a", "1"), provider("b", "2"), provider("c", "3")],
        };
        let overlay = CatalogConfig {
            schema_version: 1,
            providers: vec![provider("c", "30"), provider("d", "4")],
        };
        let merged = merge_catalogs(&base, &overlay);
        let names: Vec<&str> = merged.providers.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c", "d"]);
        let c = merged.providers.iter().find(|p| p.name == "c").unwrap();
        assert_eq!(c.base_url.as_deref(), Some("30"));
    }

    #[test]
    fn load_user_or_default_uses_builtin_when_no_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let merged = load_user_or_default(&dir.path().join("nope.toml")).unwrap();
        assert!(merged.providers.iter().any(|p| p.name == "openai"));
    }

    #[test]
    fn load_user_or_default_merges_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("catalog.toml");
        std::fs::write(
            &path,
            r#"
schema_version = 1
[[providers]]
name = "myllm"
base_url = "https://my.example"
"#,
        )
        .unwrap();
        let merged = load_user_or_default(&path).unwrap();
        assert!(merged.providers.iter().any(|p| p.name == "openai"));
        assert!(merged.providers.iter().any(|p| p.name == "myllm"));
    }

    #[test]
    fn provider_kind_dispatches_on_kind_and_api() {
        let anthropic = provider("anthropic", "x");
        let mut p = anthropic.clone();
        p.kind = Some("anthropic".into());
        assert_eq!(ProviderKind::from_provider(&p), ProviderKind::Anthropic);

        let mut p = anthropic;
        p.api = Some("openai-responses".into());
        assert_eq!(
            ProviderKind::from_provider(&p),
            ProviderKind::OpenaiResponses
        );

        let p = provider("x", "y");
        assert_eq!(
            ProviderKind::from_provider(&p),
            ProviderKind::OpenaiCompatible
        );
    }
}
