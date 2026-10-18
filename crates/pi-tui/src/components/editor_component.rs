//! `editor-component.ts` (74 lines) port: the interface for custom editor
//! components — the extension surface the `coding-agent` package builds
//! against (`EditorFactory = (tui, theme, keybindings) => EditorComponent`).
//! It allows extensions to provide their own editor implementation
//! (e.g. vim mode, emacs mode, custom keybindings) while maintaining
//! compatibility with the core application.
//!
//! TS optional members (`onSubmit?`, `addToHistory?`, …) become provided
//! methods with no-op defaults, mirroring the TS host's `?.`-guarded call
//! sites. `getExpandedText?` falls back to `getText()` exactly as its TS
//! doc comment specifies, and `borderColor?` defaults to the identity
//! function.

use std::rc::Rc;

use crate::components::autocomplete::AutocompleteProvider;
use crate::component::Component;

use super::editor::{Editor, EditorCallback};

pub trait EditorComponent: Component {
	// =========================================================================
	// Core text access (required)
	// =========================================================================

	/// `getText(): string` — the current text content.
	fn get_text(&self) -> String;

	/// `setText(text: string): void` — replace the text content.
	fn set_text(&mut self, text: &str);

	/// `handleInput(data: string): void` — raw terminal input (key
	/// presses, paste sequences, etc.).
	fn handle_input(&mut self, data: &str);

	// =========================================================================
	// Callbacks (optional properties in TS; setter methods here)
	// =========================================================================

	/// `onSubmit?: (text: string) => void` — called when the user submits
	/// (e.g. Enter key).
	fn set_on_submit(&mut self, _callback: Option<EditorCallback>) {}

	/// `onChange?: (text: string) => void` — called when the text changes.
	fn set_on_change(&mut self, _callback: Option<EditorCallback>) {}

	// =========================================================================
	// History support (optional)
	// =========================================================================

	/// `addToHistory?(text: string): void` — add text to history for
	/// up/down navigation.
	fn add_to_history(&mut self, _text: &str) {}

	// =========================================================================
	// Advanced text manipulation (optional)
	// =========================================================================

	/// `insertTextAtCursor?(text: string): void` — insert text at the
	/// current cursor position.
	fn insert_text_at_cursor(&mut self, _text: &str) {}

	/// `getExpandedText?(): string` — text with any markers expanded
	/// (e.g. paste markers). Falls back to `getText()` if not implemented.
	fn get_expanded_text(&self) -> String {
		self.get_text()
	}

	// =========================================================================
	// Autocomplete support (optional)
	// =========================================================================

	/// `setAutocompleteProvider?(provider: AutocompleteProvider): void`.
	fn set_autocomplete_provider(&mut self, _provider: Rc<dyn AutocompleteProvider>) {}

	// =========================================================================
	// Appearance (optional)
	// =========================================================================

	/// `borderColor?: (str: string) => string` — styles border text.
	/// Identity when not implemented.
	fn border_color(&self, text: &str) -> String {
		text.to_string()
	}

	/// `setPaddingX?(padding: number): void` — horizontal padding.
	fn set_padding_x(&mut self, _padding: f64) {}

	/// `setAutocompleteMaxVisible?(maxVisible: number): void` — max
	/// visible items in the autocomplete dropdown.
	fn set_autocomplete_max_visible(&mut self, _max_visible: f64) {}
}

impl EditorComponent for Editor {
	fn get_text(&self) -> String {
		Editor::get_text(self)
	}

	fn set_text(&mut self, text: &str) {
		Editor::set_text(self, text)
	}

	fn handle_input(&mut self, data: &str) {
		// `Component::handle_input` (already implemented for `Editor`)
		// delegates to `handle_input_data` in `editor_input.rs`.
		Component::handle_input(self, data)
	}

	fn set_on_submit(&mut self, callback: Option<Box<dyn FnMut(&str)>>) {
		self.on_submit = callback;
	}

	fn set_on_change(&mut self, callback: Option<Box<dyn FnMut(&str)>>) {
		self.on_change = callback;
	}

	fn add_to_history(&mut self, text: &str) {
		Editor::add_to_history(self, text)
	}

	fn insert_text_at_cursor(&mut self, text: &str) {
		Editor::insert_text_at_cursor(self, text)
	}

	fn get_expanded_text(&self) -> String {
		Editor::get_expanded_text(self)
	}

	fn set_autocomplete_provider(&mut self, provider: Rc<dyn AutocompleteProvider>) {
		Editor::set_autocomplete_provider(self, provider)
	}

	fn border_color(&self, text: &str) -> String {
		(self.border_color)(text)
	}

	fn set_padding_x(&mut self, padding: f64) {
		Editor::set_padding_x(self, padding)
	}

	fn set_autocomplete_max_visible(&mut self, max_visible: f64) {
		Editor::set_autocomplete_max_visible(self, max_visible)
	}
}
