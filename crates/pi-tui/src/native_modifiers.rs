//! Port of `native-modifiers.ts` — native modifier-key queries.
//!
//! `isNativeModifierPressed` asks the platform helper whether a modifier is
//! currently held. A missing helper (or one without `isModifierPressed`)
//! answers `false`; the TS `try/catch` around the call has no Rust
//! equivalent because the ported helpers cannot throw.

pub use crate::native_platform::ModifierKey;

use crate::native_platform::{get_native_platform_helper, NativePlatformHelper};

/// Port of `isNativeModifierPressed` with the platform helper injected for
/// tests: `None` (no helper, or a helper without modifier support) is
/// `false`, as is any answer other than an explicit `true`.
pub fn is_native_modifier_pressed_with(
    key: ModifierKey,
    helper: Option<&dyn NativePlatformHelper>,
) -> bool {
    matches!(helper.and_then(|helper| helper.is_modifier_pressed(key)), Some(true))
}

/// Port of `isNativeModifierPressed` for the real process.
pub fn is_native_modifier_pressed(key: ModifierKey) -> bool {
    is_native_modifier_pressed_with(key, get_native_platform_helper().as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_platform::{ClipboardImage, ClipboardText, NativeClipboard};

    struct StubHelper {
        pressed: Option<bool>,
    }

    impl NativeClipboard for StubHelper {
        fn get_text(&self) -> ClipboardText {
            None
        }

        fn get_image(&self) -> ClipboardImage {
            None
        }
    }

    impl NativePlatformHelper for StubHelper {
        fn is_modifier_pressed(&self, _key: ModifierKey) -> Option<bool> {
            self.pressed
        }
    }

    /// A helper without `isModifierPressed` (TS: the optional method is
    /// absent, so `helper?.isModifierPressed` is falsy).
    struct BareHelper;

    impl NativeClipboard for BareHelper {
        fn get_text(&self) -> ClipboardText {
            None
        }

        fn get_image(&self) -> ClipboardImage {
            None
        }
    }

    impl NativePlatformHelper for BareHelper {}

    #[test]
    fn returns_false_without_a_helper() {
        for key in [
            ModifierKey::Shift,
            ModifierKey::Command,
            ModifierKey::Control,
            ModifierKey::Option,
        ] {
            assert!(!is_native_modifier_pressed_with(key, None));
        }
    }

    #[test]
    fn returns_false_when_the_helper_lacks_modifier_support() {
        let helper = BareHelper;
        assert!(!is_native_modifier_pressed_with(ModifierKey::Shift, Some(&helper)));
    }

    #[test]
    fn returns_true_only_for_an_explicit_pressed_answer() {
        let pressed = StubHelper { pressed: Some(true) };
        let released = StubHelper { pressed: Some(false) };
        let unavailable = StubHelper { pressed: None };
        assert!(is_native_modifier_pressed_with(ModifierKey::Option, Some(&pressed)));
        assert!(!is_native_modifier_pressed_with(ModifierKey::Option, Some(&released)));
        assert!(!is_native_modifier_pressed_with(ModifierKey::Option, Some(&unavailable)));
    }

    #[test]
    fn real_process_has_no_native_modules() {
        let _guard = crate::test_support::global_test_lock();
        // T3/T4: the Rust port ships no N-API modules, so no modifier is
        // ever reported as pressed.
        assert!(!is_native_modifier_pressed(ModifierKey::Command));
    }
}
