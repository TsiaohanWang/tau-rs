//! ratatui-based interactive TUI for tau-rs (Phase 7).
//!
//! Layered exactly like the original `tau_coding/tui/`:
//! - `state` — pure display state (`TuiState` / `ChatItem`), no ratatui dep.
//! - `adapter` — `TuiEventAdapter::apply(&AgentEvent)` projects Pi-compatible
//!   events into `TuiState` (mirrors original `adapter.py`, no UI dep).
//! - `ui` — ratatui layout + rendering of `TuiState`.
//! - `app` — main loop: multiplexes crossterm key events with the
//!   `session.prompt` event stream via `tokio::select!`.
//!
//! Per the architecture constraint (`docs/architecture.md` §7), this module
//! only consumes `tau_types` events and the `CodingSession` read-only
//! interface — it never reaches into `tau-agent`/`tau-ai` HTTP internals. It
//! is compiled only under `feature = "tui"` (default off) to keep the plain
//! binary free of the ratatui dependency.

#[cfg(feature = "tui")]
pub mod adapter;
#[cfg(feature = "tui")]
pub mod app;
#[cfg(feature = "tui")]
pub mod state;
#[cfg(feature = "tui")]
pub mod ui;
