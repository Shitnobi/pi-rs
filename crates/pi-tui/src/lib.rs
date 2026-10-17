//! pi-tui: Rust port of `@earendil-works/pi-tui` — the terminal UI library
//! used by the pi coding agent (differential rendering, components, editor).
//!
//! Ported layer by layer with differential parity coverage against the
//! original TypeScript implementation; see `docs/TUI_PORT_PLAN.md`.

pub mod colors;
pub mod fdlibm;
pub mod fuzzy;
pub mod keys;
pub mod keybindings;
pub mod kill_ring;
pub mod native_modifiers;
pub mod native_module_path;
pub mod native_platform;
pub mod oklab;
pub mod program_status;
pub mod stdin_buffer;
pub mod terminal;
pub mod terminal_colors;
pub mod unicode;
pub mod word_navigation;
pub mod undo_stack;
pub mod utils;

#[cfg(test)]
pub mod test_support;
