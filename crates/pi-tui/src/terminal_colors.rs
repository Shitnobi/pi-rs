//! Terminal-reported colors (OSC 10/11/4 replies, color scheme reports) —
//! port of `terminal-colors.ts`.

use crate::utils::is_js_whitespace;

/// sRGB channels, 0-255. Values are `f64` because `colors.ts` color math can
/// produce fractional channels (mixing); parsed terminal replies are integers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RgbColor {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalColorScheme {
    Dark,
    Light,
}

/// Colors the terminal reports for its current theme.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TerminalColors {
    /// Default foreground (OSC 10).
    pub foreground: Option<RgbColor>,
    /// Default background (OSC 11).
    pub background: Option<RgbColor>,
    /// ANSI colors 0-15 (OSC 4). Only set when the terminal reported all 16.
    pub palette: Option<Vec<RgbColor>>,
}

fn hex_to_rgb(hex: &str) -> RgbColor {
    let normalized = hex.strip_prefix('#').unwrap_or(hex);
    let r = u32::from_str_radix(&normalized[0..2], 16).unwrap();
    let g = u32::from_str_radix(&normalized[2..4], 16).unwrap();
    let b = u32::from_str_radix(&normalized[4..6], 16).unwrap();
    RgbColor {
        r: r as f64,
        g: g as f64,
        b: b as f64,
    }
}

/// `parseOscHexChannel`: a hex string of any length, scaled to 0-255.
/// Returns `None` for empty or non-hex input.
fn parse_osc_hex_channel(channel: &str) -> Option<f64> {
    if channel.is_empty() || !channel.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let value = u64::from_str_radix(channel, 16).ok()?;
    let max = (1u64 << (4 * channel.len() as u32)) - 1;
    if max == 0 {
        return None;
    }
    Some(crate::oklab::js_round((value as f64 / max as f64) * 255.0))
}

/// What an OSC color reply reports: the default foreground (OSC 10),
/// background (OSC 11), or a palette index (OSC 4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OscColorTarget {
    Foreground,
    Background,
    Index(u32),
}

/// Port of `/^\x1b\](?:(1[01])|4;(\d{1,3}));([^\x07\x1b]*)(?:\x07|\x1b\\)$/i`.
/// Splits the raw value out of an OSC 10/11/4 reply, or returns `None` when
/// `data` is not such a reply.
fn split_osc_color_response(data: &str) -> Option<(OscColorTarget, &str)> {
    let rest = data.strip_prefix("\x1b]")?;
    // Body must not contain \x07 or \x1b, so the terminator is unambiguous.
    let (body, _terminator_len) = if let Some(b) = rest.strip_suffix("\x1b\\") {
        (b, 2)
    } else if let Some(b) = rest.strip_suffix('\x07') {
        (b, 1)
    } else {
        return None;
    };
    if body.contains('\x07') || body.contains('\x1b') {
        return None;
    }
    // `(1[01])` or `4;(\d{1,3})` as the target, then `;` and the value.
    let (target, value) = if let Some(rest) = body.strip_prefix("4;") {
        let (index_part, value) = rest.split_once(';')?;
        if index_part.is_empty()
            || index_part.len() > 3
            || !index_part.chars().all(|c| c.is_ascii_digit())
        {
            return None;
        }
        (OscColorTarget::Index(index_part.parse::<u32>().ok()?), value)
    } else {
        let (target_part, value) = body.split_once(';')?;
        let target = if target_part == "10" {
            OscColorTarget::Foreground
        } else if target_part == "11" {
            OscColorTarget::Background
        } else {
            return None;
        };
        (target, value)
    };
    Some((target, value))
}

/// Parse an OSC 10, 11, or 4 color reply. Returns `None` when `data` is not
/// such a reply; `rgb` is `None` when it is a reply with an unparseable color.
pub fn parse_osc_color_response(data: &str) -> Option<(OscColorTarget, Option<RgbColor>)> {
    let (target, value) = split_osc_color_response(data)?;
    Some((target, parse_osc_color_value(value)))
}

/// Trim using the JS whitespace set (`.trim()` in the original).
fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

fn parse_osc_color_value(raw_value: &str) -> Option<RgbColor> {
    let value = js_trim(raw_value);
    if let Some(hex) = value.strip_prefix('#') {
        if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(hex_to_rgb(value));
        }
        if hex.len() == 12 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            let r = parse_osc_hex_channel(&hex[0..4])?;
            let g = parse_osc_hex_channel(&hex[4..8])?;
            let b = parse_osc_hex_channel(&hex[8..12])?;
            return Some(RgbColor { r, g, b });
        }
        return None;
    }

    // `rgb:rrrr/gggg/bbbb` — strip an optional `rgb:`/`rgba:` prefix
    // (case-insensitive), then split on `/`.
    let lower = value.to_ascii_lowercase();
    let rgb_value = lower
        .strip_prefix("rgba:")
        .or_else(|| lower.strip_prefix("rgb:"))
        .unwrap_or(&lower);
    let mut parts = rgb_value.split('/');
    let red = parts.next()?;
    let green = parts.next()?;
    let blue = parts.next()?;
    // Extra components beyond three are ignored (JS destructuring semantics).
    let r = parse_osc_hex_channel(red)?;
    let g = parse_osc_hex_channel(green)?;
    let b = parse_osc_hex_channel(blue)?;
    Some(RgbColor { r, g, b })
}

/// Port of `/^(?:\x1b\[\?997;(1|2)n)+$/`: the report of the LAST pair wins.
/// Returns `None` when `data` is not composed solely of such pairs.
pub fn parse_terminal_color_scheme_report(data: &str) -> Option<TerminalColorScheme> {
    let mut scheme = None;
    let mut rest = data;
    loop {
        let Some(tail) = rest.strip_prefix("\x1b[?997;") else {
            return if rest.is_empty() { scheme } else { None };
        };
        let digit = tail.chars().next()?;
        if digit != '1' && digit != '2' {
            return None;
        }
        let value = &tail[1..];
        let Some(next) = value.strip_prefix('n') else {
            return None;
        };
        scheme = Some(if digit == '2' {
            TerminalColorScheme::Light
        } else {
            TerminalColorScheme::Dark
        });
        rest = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_color_scheme_reports() {
        assert_eq!(
            parse_terminal_color_scheme_report("\x1b[?997;1n"),
            Some(TerminalColorScheme::Dark)
        );
        assert_eq!(
            parse_terminal_color_scheme_report("\x1b[?997;2n"),
            Some(TerminalColorScheme::Light)
        );
        // Last pair wins.
        assert_eq!(
            parse_terminal_color_scheme_report("\x1b[?997;2n\x1b[?997;1n\x1b[?997;1n"),
            Some(TerminalColorScheme::Dark)
        );
        assert_eq!(
            parse_terminal_color_scheme_report("\x1b[?997;1n\x1b[?997;2n\x1b[?997;2n"),
            Some(TerminalColorScheme::Light)
        );
        assert_eq!(parse_terminal_color_scheme_report("\x1b[?997;3n"), None);
        assert_eq!(parse_terminal_color_scheme_report("\x1b[?996n"), None);
        assert_eq!(parse_terminal_color_scheme_report("x\x1b[?997;1n"), None);
    }

    #[test]
    fn parses_osc_10_11_and_4_replies() {
        assert_eq!(
            parse_osc_color_response("\x1b]10;rgb:ffff/ffff/ffff\x07"),
            Some((
                OscColorTarget::Foreground,
                Some(RgbColor {
                    r: 255.0,
                    g: 255.0,
                    b: 255.0
                })
            ))
        );
        assert_eq!(
            parse_osc_color_response("\x1b]4;13;#ff0080\x1b\\"),
            Some((
                OscColorTarget::Index(13),
                Some(RgbColor {
                    r: 255.0,
                    g: 0.0,
                    b: 128.0
                })
            ))
        );
        assert_eq!(
            parse_osc_color_response("\x1b]4;1;bogus\x07"),
            Some((OscColorTarget::Index(1), None))
        );
        assert_eq!(parse_osc_color_response("\x1b]12;#ffffff\x07"), None);
    }

    #[test]
    fn parses_12bit_and_scaled_osc_values() {
        assert_eq!(
            parse_osc_color_response("\x1b]10;rgb:eaea/6c6c/6d6d\x07"),
            Some((
                OscColorTarget::Foreground,
                Some(RgbColor {
                    r: 234.0,
                    g: 108.0,
                    b: 109.0
                })
            ))
        );
        assert_eq!(
            parse_osc_color_response("\x1b]11;rgb:0000/0000/0000\x1b\\"),
            Some((
                OscColorTarget::Background,
                Some(RgbColor {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0
                })
            ))
        );
    }
}
