//! Port of `packages/tui/src/latex.ts` — Unicode-text LaTeX rendering.
//! Ground truth: /tmp/pi-repo/packages/tui/src/latex.ts (1506 lines).
//!
//! Public seam (matches `export function renderLatex(source, options = {})`):
//! `render_latex(source, &RenderLatexOptions)` → `Option<String>`
//! (TS `string | undefined`).

/// TS `RenderLatexOptions` (`{ display?: boolean }`).
#[derive(Debug, Clone, Default)]
pub struct RenderLatexOptions {
    pub display: Option<bool>,
}

/// TS `renderLatex(source, options = {})` → `string | undefined`.
pub fn render_latex(source: &str, options: &RenderLatexOptions) -> Option<String> {
    let _ = (source, options);
    todo!("latex port")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "enable once the latex port lands"]
    fn render_latex_smoke() {
        assert!(render_latex("x^2", &RenderLatexOptions::default()).is_some());
    }
}
