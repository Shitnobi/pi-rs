//! Global keybinding registry — port of `packages/tui/src/keybindings.ts`.
//!
//! The TS original models the set of known binding ids as the `Keybindings`
//! interface (declaration-merged by downstream packages) and user overrides
//! as a `Partial<Keybindings>`-shaped record. This port models the same
//! shapes as:
//!
//! - [`TUI_KEYBINDINGS`] — the defaults table (`defaultKeys` + description
//!   per binding id), a 1:1 transcription of the TS `TUI_KEYBINDINGS`.
//! - [`KeybindingsConfig`] — user claims; `None` mirrors an absent property
//!   in the TS `KeybindingsConfig` / `Partial<Keybindings>` object (the
//!   binding then keeps its defaults).
//! - [`KeybindingsManager`] — resolves defaults overlaid with user claims,
//!   detects conflicts between user claims, and matches raw input data
//!   against resolved keys via [`crate::keys::matches_key`].
//! - [`set_keybindings`] / [`get_keybindings`] — the module-global manager
//!   (TS `globalKeybindings` + accessors), lazily constructed from
//!   `TUI_KEYBINDINGS` on first use.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::keys::matches_key;

// =============================================================================
// Keybindings defaults table (port of TUI_KEYBINDINGS)
// =============================================================================

/// One entry of the [`TUI_KEYBINDINGS`] defaults table.
///
/// Port of the TS `KeybindingDefinition` records inside `TUI_KEYBINDINGS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiKeybinding {
    pub id: &'static str,
    pub default_keys: &'static [&'static str],
    pub description: &'static str,
}

/// Default TUI keybindings — 1:1 port of the TS `TUI_KEYBINDINGS` const.
///
/// Note: the altScreen bindings intentionally shadow the unmodified editor
/// bindings in fullscreen mode (see the TS comment on the altScreen block).
pub const TUI_KEYBINDINGS: &[TuiKeybinding] = &[
    // Editor navigation and editing
    TuiKeybinding {
        id: "tui.editor.cursorUp",
        default_keys: &["up"],
        description: "Move cursor up",
    },
    TuiKeybinding {
        id: "tui.editor.cursorDown",
        default_keys: &["down"],
        description: "Move cursor down",
    },
    TuiKeybinding {
        id: "tui.editor.historyPrevious",
        default_keys: &[],
        description: "Select previous prompt history entry",
    },
    TuiKeybinding {
        id: "tui.editor.historyNext",
        default_keys: &[],
        description: "Select next prompt history entry",
    },
    TuiKeybinding {
        id: "tui.editor.cursorLeft",
        default_keys: &["left", "ctrl+b"],
        description: "Move cursor left",
    },
    TuiKeybinding {
        id: "tui.editor.cursorRight",
        default_keys: &["right", "ctrl+f"],
        description: "Move cursor right",
    },
    TuiKeybinding {
        id: "tui.editor.cursorWordLeft",
        default_keys: &["alt+left", "ctrl+left", "alt+b"],
        description: "Move cursor word left",
    },
    TuiKeybinding {
        id: "tui.editor.cursorWordRight",
        default_keys: &["alt+right", "ctrl+right", "alt+f"],
        description: "Move cursor word right",
    },
    TuiKeybinding {
        id: "tui.editor.cursorLineStart",
        default_keys: &["home", "ctrl+a"],
        description: "Move to line start",
    },
    TuiKeybinding {
        id: "tui.editor.cursorLineEnd",
        default_keys: &["end", "ctrl+e"],
        description: "Move to line end",
    },
    TuiKeybinding {
        id: "tui.editor.jumpForward",
        default_keys: &["ctrl+]"],
        description: "Jump forward to character",
    },
    TuiKeybinding {
        id: "tui.editor.jumpBackward",
        default_keys: &["ctrl+alt+]"],
        description: "Jump backward to character",
    },
    TuiKeybinding {
        id: "tui.editor.pageUp",
        default_keys: &["pageUp", "ctrl+pageUp"],
        description: "Page up",
    },
    TuiKeybinding {
        id: "tui.editor.pageDown",
        default_keys: &["pageDown", "ctrl+pageDown"],
        description: "Page down",
    },
    TuiKeybinding {
        id: "tui.editor.deleteCharBackward",
        default_keys: &["backspace"],
        description: "Delete character backward",
    },
    TuiKeybinding {
        id: "tui.editor.deleteCharForward",
        default_keys: &["delete", "ctrl+d"],
        description: "Delete character forward",
    },
    TuiKeybinding {
        id: "tui.editor.deleteWordBackward",
        default_keys: &["ctrl+w", "alt+backspace"],
        description: "Delete word backward",
    },
    TuiKeybinding {
        id: "tui.editor.deleteWordForward",
        default_keys: &["alt+d", "alt+delete"],
        description: "Delete word forward",
    },
    TuiKeybinding {
        id: "tui.editor.deleteToLineStart",
        default_keys: &["ctrl+u"],
        description: "Delete to line start",
    },
    TuiKeybinding {
        id: "tui.editor.deleteToLineEnd",
        default_keys: &["ctrl+k"],
        description: "Delete to line end",
    },
    TuiKeybinding {
        id: "tui.editor.yank",
        default_keys: &["ctrl+y"],
        description: "Yank",
    },
    TuiKeybinding {
        id: "tui.editor.yankPop",
        default_keys: &["alt+y"],
        description: "Yank pop",
    },
    TuiKeybinding {
        id: "tui.editor.undo",
        default_keys: &["ctrl+-"],
        description: "Undo",
    },
    // Generic input actions
    TuiKeybinding {
        id: "tui.input.newLine",
        default_keys: &["shift+enter", "ctrl+j"],
        description: "Insert newline",
    },
    TuiKeybinding {
        id: "tui.input.submit",
        default_keys: &["enter"],
        description: "Submit input",
    },
    TuiKeybinding {
        id: "tui.input.tab",
        default_keys: &["tab"],
        description: "Tab / autocomplete",
    },
    TuiKeybinding {
        id: "tui.input.copy",
        default_keys: &["ctrl+c"],
        description: "Copy selection",
    },
    // Generic selection actions
    TuiKeybinding {
        id: "tui.select.up",
        default_keys: &["up"],
        description: "Move selection up",
    },
    TuiKeybinding {
        id: "tui.select.down",
        default_keys: &["down"],
        description: "Move selection down",
    },
    TuiKeybinding {
        id: "tui.select.pageUp",
        default_keys: &["pageUp"],
        description: "Selection page up",
    },
    TuiKeybinding {
        id: "tui.select.pageDown",
        default_keys: &["pageDown"],
        description: "Selection page down",
    },
    TuiKeybinding {
        id: "tui.select.confirm",
        default_keys: &["enter"],
        description: "Confirm selection",
    },
    TuiKeybinding {
        id: "tui.select.cancel",
        default_keys: &["escape", "ctrl+c"],
        description: "Cancel selection",
    },
    // Alternate-screen viewport navigation.
    // These intentionally shadow the unmodified editor bindings in fullscreen
    // mode (mirrors the TS comment).
    TuiKeybinding {
        id: "tui.altScreen.pageUp",
        default_keys: &["pageUp"],
        description: "Scroll viewport up one page",
    },
    TuiKeybinding {
        id: "tui.altScreen.pageDown",
        default_keys: &["pageDown"],
        description: "Scroll viewport down one page",
    },
    TuiKeybinding {
        id: "tui.altScreen.halfPageUp",
        default_keys: &[],
        description: "Scroll viewport up half a page",
    },
    TuiKeybinding {
        id: "tui.altScreen.halfPageDown",
        default_keys: &[],
        description: "Scroll viewport down half a page",
    },
    TuiKeybinding {
        id: "tui.altScreen.lineUp",
        default_keys: &[],
        description: "Scroll viewport up one line",
    },
    TuiKeybinding {
        id: "tui.altScreen.lineDown",
        default_keys: &[],
        description: "Scroll viewport down one line",
    },
    TuiKeybinding {
        id: "tui.altScreen.previousPrompt",
        default_keys: &["ctrl+shift+up", "ctrl+up"],
        description: "Jump to previous semantic prompt",
    },
    TuiKeybinding {
        id: "tui.altScreen.nextPrompt",
        default_keys: &["ctrl+shift+down", "ctrl+down"],
        description: "Jump to next semantic prompt",
    },
    TuiKeybinding {
        id: "tui.altScreen.search",
        default_keys: &["ctrl+shift+f"],
        description: "Search the primary scroll view",
    },
    TuiKeybinding {
        id: "tui.altScreen.searchNext",
        default_keys: &["enter", "ctrl+g"],
        description: "Select the next search match",
    },
    TuiKeybinding {
        id: "tui.altScreen.searchPrevious",
        default_keys: &["shift+enter", "ctrl+shift+g"],
        description: "Select the previous search match",
    },
    TuiKeybinding {
        id: "tui.altScreen.searchClose",
        default_keys: &["escape"],
        description: "Close transcript search",
    },
    TuiKeybinding {
        id: "tui.altScreen.top",
        default_keys: &["ctrl+home"],
        description: "Scroll viewport to top",
    },
    TuiKeybinding {
        id: "tui.altScreen.bottom",
        default_keys: &["ctrl+end"],
        description: "Scroll viewport to bottom",
    },
];

// =============================================================================
// User keybinding claims (port of KeybindingsConfig / Partial<Keybindings>)
// =============================================================================

/// Declares the typed user-claims struct, one optional field per binding id.
///
/// The TS `Keybindings` interface lists every known binding id; user overrides
/// arrive as a `Partial<Keybindings>`-shaped record where an absent property
/// means "keep the defaults". `None` here plays the role of the absent
/// property. Field order matches the TS declaration order, which drives the
/// claim iteration order in [`KeybindingsManager`] (mirroring TS
/// `Object.entries` insertion order for object literals).
macro_rules! define_keybindings_config {
    ( $( $field:ident , $id:literal ; )* ) => {
        /// User keybinding claims — port of the TS `KeybindingsConfig`
        /// (`Record<string, KeyId | KeyId[] | undefined>` / `Partial<Keybindings>`).
        ///
        /// `None` mirrors an absent property: the binding keeps its defaults.
        /// The TS distinguishes a single `KeyId` from a `KeyId[]`; both are
        /// normalized identically by `normalizeKeys`, so this port stores the
        /// list form only.
        #[derive(Debug, Default, Clone, PartialEq, Eq)]
        pub struct KeybindingsConfig {
            $( pub $field: Option<Vec<String>>, )*
        }

        impl KeybindingsConfig {
            /// All binding ids in TS declaration order with their claimed
            /// keys (`None` when unclaimed).
            pub fn entries(&self) -> Vec<(&'static str, Option<&[String]>)> {
                vec![ $( ($id, self.$field.as_deref()), )* ]
            }

            /// Claimed keys for a binding id (`None` when unclaimed).
            pub fn get(&self, id: &str) -> Option<&Vec<String>> {
                match id {
                    $( $id => self.$field.as_ref(), )*
                    _ => None,
                }
            }

            /// Set the claim for a binding id (no-op for unknown ids, which
            /// the TS skips during `rebuild` via its `in this.definitions`
            /// check — the typed struct cannot even hold them).
            fn set(&mut self, id: &str, keys: Vec<String>) {
                match id {
                    $( $id => { self.$field = Some(keys); } )*
                    _ => {}
                }
            }
        }
    };
}

define_keybindings_config! {
    tui_editor_cursor_up, "tui.editor.cursorUp";
    tui_editor_cursor_down, "tui.editor.cursorDown";
    tui_editor_history_previous, "tui.editor.historyPrevious";
    tui_editor_history_next, "tui.editor.historyNext";
    tui_editor_cursor_left, "tui.editor.cursorLeft";
    tui_editor_cursor_right, "tui.editor.cursorRight";
    tui_editor_cursor_word_left, "tui.editor.cursorWordLeft";
    tui_editor_cursor_word_right, "tui.editor.cursorWordRight";
    tui_editor_cursor_line_start, "tui.editor.cursorLineStart";
    tui_editor_cursor_line_end, "tui.editor.cursorLineEnd";
    tui_editor_jump_forward, "tui.editor.jumpForward";
    tui_editor_jump_backward, "tui.editor.jumpBackward";
    tui_editor_page_up, "tui.editor.pageUp";
    tui_editor_page_down, "tui.editor.pageDown";
    tui_editor_delete_char_backward, "tui.editor.deleteCharBackward";
    tui_editor_delete_char_forward, "tui.editor.deleteCharForward";
    tui_editor_delete_word_backward, "tui.editor.deleteWordBackward";
    tui_editor_delete_word_forward, "tui.editor.deleteWordForward";
    tui_editor_delete_to_line_start, "tui.editor.deleteToLineStart";
    tui_editor_delete_to_line_end, "tui.editor.deleteToLineEnd";
    tui_editor_yank, "tui.editor.yank";
    tui_editor_yank_pop, "tui.editor.yankPop";
    tui_editor_undo, "tui.editor.undo";
    tui_input_new_line, "tui.input.newLine";
    tui_input_submit, "tui.input.submit";
    tui_input_tab, "tui.input.tab";
    tui_input_copy, "tui.input.copy";
    tui_select_up, "tui.select.up";
    tui_select_down, "tui.select.down";
    tui_select_page_up, "tui.select.pageUp";
    tui_select_page_down, "tui.select.pageDown";
    tui_select_confirm, "tui.select.confirm";
    tui_select_cancel, "tui.select.cancel";
    tui_alt_screen_page_up, "tui.altScreen.pageUp";
    tui_alt_screen_page_down, "tui.altScreen.pageDown";
    tui_alt_screen_half_page_up, "tui.altScreen.halfPageUp";
    tui_alt_screen_half_page_down, "tui.altScreen.halfPageDown";
    tui_alt_screen_line_up, "tui.altScreen.lineUp";
    tui_alt_screen_line_down, "tui.altScreen.lineDown";
    tui_alt_screen_previous_prompt, "tui.altScreen.previousPrompt";
    tui_alt_screen_next_prompt, "tui.altScreen.nextPrompt";
    tui_alt_screen_search, "tui.altScreen.search";
    tui_alt_screen_search_next, "tui.altScreen.searchNext";
    tui_alt_screen_search_previous, "tui.altScreen.searchPrevious";
    tui_alt_screen_search_close, "tui.altScreen.searchClose";
    tui_alt_screen_top, "tui.altScreen.top";
    tui_alt_screen_bottom, "tui.altScreen.bottom";
}

// =============================================================================
// Definitions & manager
// =============================================================================

/// Owned definition record — port of the TS `KeybindingDefinition`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindingDefinition {
    pub default_keys: Vec<String>,
    pub description: Option<String>,
}

/// Ordered definition table — port of the TS `KeybindingDefinitions`
/// (`Record<string, KeybindingDefinition>`); a `Vec` of `(id, definition)`
/// pairs preserves the TS object-literal iteration order.
pub type KeybindingDefinitions = Vec<(String, KeybindingDefinition)>;

/// Materialize [`TUI_KEYBINDINGS`] into owned [`KeybindingDefinitions`].
pub fn tui_keybinding_definitions() -> KeybindingDefinitions {
    TUI_KEYBINDINGS
        .iter()
        .map(|entry| {
            (
                entry.id.to_string(),
                KeybindingDefinition {
                    default_keys: entry
                        .default_keys
                        .iter()
                        .map(|key| key.to_string())
                        .collect(),
                    description: Some(entry.description.to_string()),
                },
            )
        })
        .collect()
}

/// A key claimed by more than one user binding — port of the TS
/// `KeybindingConflict`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindingConflict {
    pub key: String,
    pub keybindings: Vec<String>,
}

/// Port of the TS `normalizeKeys`: `undefined` → `[]`, single key → wrapped,
/// and dedup preserving first-occurrence order.
fn normalize_keys(keys: Option<&[String]>) -> Vec<String> {
    let Some(keys) = keys else {
        return Vec::new();
    };
    let mut seen: std::collections::HashSet<&String> = std::collections::HashSet::new();
    let mut result: Vec<String> = Vec::new();
    for key in keys {
        if seen.insert(key) {
            result.push(key.clone());
        }
    }
    result
}

/// Resolved-keybinding manager — port of the TS `KeybindingsManager`.
///
/// Holds a definitions table, user claims, the rebuilt `keysById` map
/// (binding id → list of key strings), and the conflicts between user
/// claims (key strings claimed by more than one binding).
pub struct KeybindingsManager {
    definitions: KeybindingDefinitions,
    user_bindings: KeybindingsConfig,
    keys_by_id: HashMap<String, Vec<String>>,
    conflicts: Vec<KeybindingConflict>,
}

impl KeybindingsManager {
    /// Port of the TS constructor; immediately rebuilds the resolved tables.
    pub fn new(definitions: KeybindingDefinitions, user_bindings: KeybindingsConfig) -> Self {
        let mut manager = Self {
            definitions,
            user_bindings,
            keys_by_id: HashMap::new(),
            conflicts: Vec::new(),
        };
        manager.rebuild();
        manager
    }

    /// Port of the private TS `rebuild()`:
    ///
    /// 1. Collect user claims (only for ids present in the definitions),
    ///    recording for each claimed key string the set of bindings claiming
    ///    it, in claim order.
    /// 2. Keys claimed by more than one binding become conflicts.
    /// 3. Resolve `keysById`: user keys fully replace the defaults when
    ///    present; otherwise the definition defaults are used.
    fn rebuild(&mut self) {
        self.keys_by_id.clear();
        self.conflicts = Vec::new();

        // Map<KeyId, Set<Keybinding>> with insertion order preserved on both
        // levels (TS Map/Set iteration order).
        let mut user_claims: Vec<(String, Vec<String>)> = Vec::new();
        for (keybinding, keys) in self.user_bindings.entries() {
            let Some(keys) = keys else {
                continue;
            };
            if !self.definitions.iter().any(|(id, _)| id == keybinding) {
                continue;
            }
            let keybinding = keybinding.to_string();
            for key in normalize_keys(Some(keys)) {
                match user_claims.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, claimants)) => {
                        if !claimants.contains(&keybinding) {
                            claimants.push(keybinding.clone());
                        }
                    }
                    None => user_claims.push((key, vec![keybinding.clone()])),
                }
            }
        }

        for (key, keybindings) in user_claims {
            if keybindings.len() > 1 {
                self.conflicts.push(KeybindingConflict {
                    key,
                    keybindings,
                });
            }
        }

        for (id, definition) in &self.definitions {
            let keys = match self.user_bindings.get(id) {
                Some(user_keys) => normalize_keys(Some(user_keys)),
                None => normalize_keys(Some(&definition.default_keys)),
            };
            self.keys_by_id.insert(id.clone(), keys);
        }
    }

    /// Port of the TS `matches(data, keybinding)`: true when the input data
    /// matches any resolved key for the binding (via `matchesKey`).
    pub fn matches(&self, data: &str, keybinding: &str) -> bool {
        let Some(keys) = self.keys_by_id.get(keybinding) else {
            return false;
        };
        for key in keys {
            if matches_key(data, key) {
                return true;
            }
        }
        false
    }

    /// Port of the TS `getKeys(keybinding)` — a copy of the resolved keys.
    pub fn get_keys(&self, keybinding: &str) -> Vec<String> {
        self.keys_by_id
            .get(keybinding)
            .cloned()
            .unwrap_or_default()
    }

    /// Port of the TS `getDefinition(keybinding)`.
    pub fn get_definition(&self, keybinding: &str) -> Option<&KeybindingDefinition> {
        self.definitions
            .iter()
            .find(|(id, _)| id == keybinding)
            .map(|(_, definition)| definition)
    }

    /// Port of the TS `getConflicts()` — a deep copy.
    pub fn get_conflicts(&self) -> Vec<KeybindingConflict> {
        self.conflicts.clone()
    }

    /// Port of the TS `setUserBindings(userBindings)` — replaces all claims
    /// and rebuilds.
    pub fn set_user_bindings(&mut self, user_bindings: KeybindingsConfig) {
        self.user_bindings = user_bindings;
        self.rebuild();
    }

    /// Port of the TS `getUserBindings()` — a copy of the current claims.
    pub fn get_user_bindings(&self) -> KeybindingsConfig {
        self.user_bindings.clone()
    }

    /// Port of the TS `getResolvedBindings()`: the defaults overlaid with
    /// user claims, one entry per known binding id.
    ///
    /// Divergence: the TS emits a bare `KeyId` when a binding resolves to
    /// exactly one key and an array otherwise; this port stores the list form
    /// for both (the two are indistinguishable after `normalizeKeys`).
    pub fn get_resolved_bindings(&self) -> KeybindingsConfig {
        let mut resolved = KeybindingsConfig::default();
        for (id, _) in &self.definitions {
            if let Some(keys) = self.keys_by_id.get(id) {
                resolved.set(id, keys.clone());
            }
        }
        resolved
    }
}

// =============================================================================
// Global manager (port of globalKeybindings + setKeybindings/getKeybindings)
// =============================================================================

static GLOBAL_KEYBINDINGS: RwLock<Option<Arc<KeybindingsManager>>> = RwLock::new(None);

/// Port of the TS `setKeybindings(keybindings)` — install a global manager.
pub fn set_keybindings(keybindings: KeybindingsManager) {
    let mut guard = GLOBAL_KEYBINDINGS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = Some(Arc::new(keybindings));
}

/// Port of the TS `getKeybindings()` — lazily constructs the default manager
/// (`new KeybindingsManager(TUI_KEYBINDINGS)`) on first use. Returns a shared
/// handle (`Arc`) instead of a reference.
pub fn get_keybindings() -> Arc<KeybindingsManager> {
    let mut guard = GLOBAL_KEYBINDINGS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard
        .get_or_insert_with(|| {
            Arc::new(KeybindingsManager::new(
                tui_keybinding_definitions(),
                KeybindingsConfig::default(),
            ))
        })
        .clone()
}

// =============================================================================
// Tests — port of packages/tui/test/keybindings.test.ts
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|key| key.to_string()).collect()
    }

    // describe("KeybindingsManager")

    // it("binds Ctrl+J as a default newline alias")
    #[test]
    fn binds_ctrl_j_as_a_default_newline_alias() {
        // matches() consults the global kitty-protocol flag via
        // `matches_key`; hold the crate-wide lock so parallel keys.rs tests
        // cannot flip it mid-assertion (the TS suite runs sequentially).
        let _guard = crate::test_support::global_test_lock();

        let keybindings = KeybindingsManager::new(
            tui_keybinding_definitions(),
            KeybindingsConfig::default(),
        );

        assert_eq!(
            keybindings.get_keys("tui.input.newLine"),
            keys(&["shift+enter", "ctrl+j"])
        );
        assert!(keybindings.matches("\n", "tui.input.newLine"));
        assert!(keybindings.matches("\x1b[106;5u", "tui.input.newLine"));
    }

    // it("binds modified and unmodified editor viewport navigation")
    #[test]
    fn binds_modified_and_unmodified_editor_viewport_navigation() {
        let keybindings = KeybindingsManager::new(
            tui_keybinding_definitions(),
            KeybindingsConfig::default(),
        );

        assert_eq!(
            keybindings.get_keys("tui.editor.cursorLineStart"),
            keys(&["home", "ctrl+a"])
        );
        assert_eq!(
            keybindings.get_keys("tui.editor.cursorLineEnd"),
            keys(&["end", "ctrl+e"])
        );
        assert_eq!(
            keybindings.get_keys("tui.editor.pageUp"),
            keys(&["pageUp", "ctrl+pageUp"])
        );
        assert_eq!(
            keybindings.get_keys("tui.editor.pageDown"),
            keys(&["pageDown", "ctrl+pageDown"])
        );
    }

    // it("leaves dedicated prompt history navigation unbound by default")
    #[test]
    fn leaves_dedicated_prompt_history_navigation_unbound_by_default() {
        let keybindings = KeybindingsManager::new(
            tui_keybinding_definitions(),
            KeybindingsConfig::default(),
        );

        assert_eq!(keybindings.get_keys("tui.editor.historyPrevious"), Vec::<String>::new());
        assert_eq!(keybindings.get_keys("tui.editor.historyNext"), Vec::<String>::new());
    }

    // it("binds unmodified terminal viewport shortcuts to alternate-screen navigation")
    #[test]
    fn binds_unmodified_terminal_viewport_shortcuts_to_alternate_screen_navigation() {
        let keybindings = KeybindingsManager::new(
            tui_keybinding_definitions(),
            KeybindingsConfig::default(),
        );

        assert_eq!(
            keybindings.get_keys("tui.altScreen.pageUp"),
            keys(&["pageUp"])
        );
        assert_eq!(
            keybindings.get_keys("tui.altScreen.pageDown"),
            keys(&["pageDown"])
        );
        assert_eq!(
            keybindings.get_keys("tui.altScreen.halfPageUp"),
            Vec::<String>::new()
        );
        assert_eq!(
            keybindings.get_keys("tui.altScreen.halfPageDown"),
            Vec::<String>::new()
        );
        assert_eq!(
            keybindings.get_keys("tui.altScreen.lineUp"),
            Vec::<String>::new()
        );
        assert_eq!(
            keybindings.get_keys("tui.altScreen.lineDown"),
            Vec::<String>::new()
        );
        assert_eq!(
            keybindings.get_keys("tui.altScreen.previousPrompt"),
            keys(&["ctrl+shift+up", "ctrl+up"])
        );
        assert_eq!(
            keybindings.get_keys("tui.altScreen.nextPrompt"),
            keys(&["ctrl+shift+down", "ctrl+down"])
        );
        assert_eq!(
            keybindings.get_keys("tui.altScreen.search"),
            keys(&["ctrl+shift+f"])
        );
        assert_eq!(
            keybindings.get_keys("tui.altScreen.searchNext"),
            keys(&["enter", "ctrl+g"])
        );
        assert_eq!(
            keybindings.get_keys("tui.altScreen.searchPrevious"),
            keys(&["shift+enter", "ctrl+shift+g"])
        );
        assert_eq!(
            keybindings.get_keys("tui.altScreen.searchClose"),
            keys(&["escape"])
        );
        assert_eq!(
            keybindings.get_keys("tui.altScreen.top"),
            keys(&["ctrl+home"])
        );
        assert_eq!(
            keybindings.get_keys("tui.altScreen.bottom"),
            keys(&["ctrl+end"])
        );
    }

    // it("does not evict selector confirm when input submit is rebound")
    #[test]
    fn does_not_evict_selector_confirm_when_input_submit_is_rebound() {
        let keybindings = KeybindingsManager::new(
            tui_keybinding_definitions(),
            KeybindingsConfig {
                tui_input_submit: Some(keys(&["enter", "ctrl+enter"])),
                ..KeybindingsConfig::default()
            },
        );

        assert_eq!(
            keybindings.get_keys("tui.input.submit"),
            keys(&["enter", "ctrl+enter"])
        );
        assert_eq!(keybindings.get_keys("tui.select.confirm"), keys(&["enter"]));
    }

    // it("does not evict cursor bindings when another action reuses the same key")
    #[test]
    fn does_not_evict_cursor_bindings_when_another_action_reuses_the_same_key() {
        let keybindings = KeybindingsManager::new(
            tui_keybinding_definitions(),
            KeybindingsConfig {
                tui_select_up: Some(keys(&["up", "ctrl+p"])),
                ..KeybindingsConfig::default()
            },
        );

        assert_eq!(keybindings.get_keys("tui.select.up"), keys(&["up", "ctrl+p"]));
        assert_eq!(keybindings.get_keys("tui.editor.cursorUp"), keys(&["up"]));
    }

    // it("still reports direct user binding conflicts without evicting defaults")
    #[test]
    fn still_reports_direct_user_binding_conflicts_without_evicting_defaults() {
        let keybindings = KeybindingsManager::new(
            tui_keybinding_definitions(),
            KeybindingsConfig {
                tui_input_submit: Some(keys(&["ctrl+x"])),
                tui_select_confirm: Some(keys(&["ctrl+x"])),
                ..KeybindingsConfig::default()
            },
        );

        assert_eq!(
            keybindings.get_conflicts(),
            vec![KeybindingConflict {
                key: "ctrl+x".to_string(),
                keybindings: vec![
                    "tui.input.submit".to_string(),
                    "tui.select.confirm".to_string(),
                ],
            }]
        );
        assert_eq!(
            keybindings.get_keys("tui.editor.cursorLeft"),
            keys(&["left", "ctrl+b"])
        );
    }

    // Supplementary coverage (not in the upstream test file) for the global
    // accessors: lazy default construction + set_keybindings override.
    #[test]
    fn global_accessors_install_and_lazily_default() {
        let _guard = crate::test_support::global_test_lock();

        let defaults = get_keybindings();
        assert_eq!(
            defaults.get_keys("tui.input.newLine"),
            keys(&["shift+enter", "ctrl+j"])
        );
        // Repeated access returns the same lazily-constructed manager.
        assert!(Arc::ptr_eq(&defaults, &get_keybindings()));

        let custom = KeybindingsManager::new(
            tui_keybinding_definitions(),
            KeybindingsConfig {
                tui_input_submit: Some(keys(&["ctrl+x"])),
                ..KeybindingsConfig::default()
            },
        );
        set_keybindings(custom);
        let installed = get_keybindings();
        assert_eq!(installed.get_keys("tui.input.submit"), keys(&["ctrl+x"]));

        // Restore the default global manager for any later consumer.
        set_keybindings(KeybindingsManager::new(
            tui_keybinding_definitions(),
            KeybindingsConfig::default(),
        ));
    }
}
