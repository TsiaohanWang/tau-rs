//! Public re-exports of the provider contract implemented by Tau adapters.

// The Python facade carries `# ruff: noqa: F401`; nothing in the crate consumes
// these re-exports yet.
#![allow(unused_imports)]

pub(crate) use crate::tau_agent::provider::{CancellationToken, ModelProvider};
