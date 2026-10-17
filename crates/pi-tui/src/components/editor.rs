//! Port of `packages/tui/src/components/editor.ts` (2473 LOC) — the
//! `Editor` component: multi-line text editing with word wrap, paste
//! markers, prompt history, kill ring, undo, character jump mode, sticky
//! column, and autocomplete.
//!
//! ## Conventions
//!
//! - **UTF-16 code-unit indexing**: `cursor_col` and all text positions are
//!   UTF-16 units (JS `String` semantics) — see `word_navigation.rs`.
//! - The `tui` host is [`crate::component::TuiHost`] (the only `TUI`
//!   members editor.ts consumes).
//! - Pure helpers (`is_paste_marker`, `segment_*_with_markers`,
//!   `word_wrap_line`) live in [`super::editor_word_wrap`].
//!
//! **Owner: T4 delegate D1a (core) then D1b (autocomplete).**
