//! pi-tui: Rust port of `@earendil-works/pi-tui` — the terminal UI library
//! used by the pi coding agent (differential rendering, components, editor).
//!
//! Ported layer by layer with differential parity coverage against the
//! original TypeScript implementation; see `docs/TUI_PORT_PLAN.md`.

pub mod colors;
pub mod fdlibm;
pub mod fuzzy;
pub mod kill_ring;
pub mod oklab;
pub mod program_status;
pub mod terminal_colors;
pub mod unicode;
pub mod undo_stack;
pub mod utils;
