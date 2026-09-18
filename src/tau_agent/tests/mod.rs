//! Test-only modules for the portable agent layer.
//!
//! Production code lives in the sibling files (`messages.rs`,
//! `provider_events.rs`, `tools.rs`, `provider.rs`); everything under this
//! directory is compiled only for `cargo test`. Keeping the tests together
//! makes the rewrite itself easy to read and review.

pub(crate) mod differential;
pub(crate) mod messages;
pub(crate) mod provider;
pub(crate) mod provider_events;
pub(crate) mod support;
pub(crate) mod tools;
