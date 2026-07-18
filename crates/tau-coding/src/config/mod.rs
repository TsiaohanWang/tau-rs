//! Catalog configuration types and deep-merge.
//!
//! `CatalogConfig` mirrors the shape loaded from `catalog.toml`. The merge
//! function combines a built-in base catalog with a user overlay: overlay
//! providers override same-named entries in base, unique providers on either
//! side are preserved, and the resulting `schema_version` is taken from the
//! overlay (falling back to the base).

pub mod catalog;

pub use catalog::{CatalogConfig, CatalogProvider, ProviderKind, merge_catalogs};
