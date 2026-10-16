//! Color values, parsing, mixing, and ANSI emission — port of `colors.ts`.

use crate::oklab::{
    js_round, linear_srgb_to_rgb, okhsl_to_rgb, oklab_to_linear_srgb, rgb_to_oklab,
    rgb_to_okhsl,
};
use crate::terminal_colors::RgbColor;

/// A concrete color. Every color can be converted to sRGB, so color math
/// never fails.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Color {
    Indexed { index: u8 },
    Rgb { r: f64, g: f64, b: f64 },
    Oklch { l: f64, c: f64, h: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalColorMode {
    Color256,
    TrueColor,
}

impl TerminalColorMode {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "256color" => TerminalColorMode::Color256,
            "truecolor" => TerminalColorMode::TrueColor,
            _ => return None,
        })
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            TerminalColorMode::Color256 => "256color",
            TerminalColorMode::TrueColor => "truecolor",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMixSpace {
    Oklch,
    Srgb,
}

/// OKLCH channels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OklchChannels {
    pub l: f64,
    pub c: f64,
    pub h: f64,
}

/// OKHSL channels: hue in degrees, saturation and lightness 0-1. Saturation
/// is relative to the most the sRGB gamut allows at that hue and lightness,
/// so every value is in gamut.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OkhslChannels {
    pub h: f64,
    pub s: f64,
    pub l: f64,
}

/// Port of the TS `TextAttributes` (all default false).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextAttributes {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub strikethrough: bool,
}

/// Port of the TS `TextStyle`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TextStyle {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub strikethrough: bool,
}

impl TextStyle {
    pub fn attributes(&self) -> TextAttributes {
        TextAttributes {
            bold: self.bold,
            dim: self.dim,
            italic: self.italic,
            underline: self.underline,
            inverse: self.inverse,
            strikethrough: self.strikethrough,
        }
    }
}

/// Port of `indexedColor`. Errors mirror the TS throw messages.
pub fn indexed_color(index: f64) -> Result<Color, String> {
    if !is_js_integer(index) || !(0.0..=255.0).contains(&index) {
        return Err(format!(
            "ANSI color index must be an integer from 0 to 255: {}",
            js_number_to_string(index)
        ));
    }
    Ok(Color::Indexed {
        index: index as u8,
    })
}

/// JS `Number.isInteger` (JSON inputs are always finite).
fn is_js_integer(value: f64) -> bool {
    value.is_finite() && value.fract() == 0.0
}

/// JS `String(value)` for the finite numbers that can reach error messages.
fn js_number_to_string(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e21 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// Port of `rgbColor`.
pub fn rgb_color(r: f64, g: f64, b: f64) -> Result<Color, String> {
    for (name, value) in [("r", r), ("g", g), ("b", b)] {
        if !value.is_finite() {
            return Err(format!("{name} must be finite"));
        }
        if !(0.0..=255.0).contains(&value) {
            return Err(format!(
                "{name} must be between 0 and 255: {}",
                js_number_to_string(value)
            ));
        }
    }
    Ok(Color::Rgb { r, g, b })
}

/// Port of `oklchColor`.
pub fn oklch_color(l: f64, c: f64, h: f64) -> Result<Color, String> {
    if !l.is_finite() {
        return Err("l must be finite".to_string());
    }
    if !c.is_finite() {
        return Err("c must be finite".to_string());
    }
    if !h.is_finite() {
        return Err("h must be finite".to_string());
    }
    if !(0.0..=1.0).contains(&l) {
        return Err(format!("l must be between 0 and 1: {}", js_number_to_string(l)));
    }
    if c < 0.0 {
        return Err(format!("c must not be negative: {}", js_number_to_string(c)));
    }
    Ok(Color::Oklch {
        l,
        c,
        h: ((h % 360.0) + 360.0) % 360.0,
    })
}

/// Port of `okhslColor`: an OKHSL color, converted to sRGB.
pub fn okhsl_color(h: f64, s: f64, l: f64) -> Result<Color, String> {
    if !h.is_finite() {
        return Err("h must be finite".to_string());
    }
    if !s.is_finite() {
        return Err("s must be finite".to_string());
    }
    if !l.is_finite() {
        return Err("l must be finite".to_string());
    }
    if !(0.0..=1.0).contains(&s) {
        return Err(format!("s must be between 0 and 1: {}", js_number_to_string(s)));
    }
    if !(0.0..=1.0).contains(&l) {
        return Err(format!("l must be between 0 and 1: {}", js_number_to_string(l)));
    }
    let RgbColor { r, g, b } = okhsl_to_rgb(h, s, l);
    rgb_color(r, g, b)
}

pub fn color_to_okhsl(color: Color) -> OkhslChannels {
    let (h, s, l) = rgb_to_okhsl(color_to_rgb(color));
    OkhslChannels { h, s, l }
}

// --- parseColor ---------------------------------------------------------------
//
// NUMBER_PATTERN = `[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?` (case
// insensitive: the exponent `e` may be `E`). Whitespace is the JS `\s` set.

struct ColorScan<'a> {
    chars: std::str::Chars<'a>,
    peeked: Option<Option<char>>,
}

impl<'a> ColorScan<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            chars: s.chars(),
            peeked: None,
        }
    }

    fn peek(&mut self) -> Option<char> {
        if self.peeked.is_none() {
            self.peeked = Some(self.chars.next());
        }
        self.peeked.unwrap()
    }

    fn bump(&mut self) -> Option<char> {
        match self.peeked.take() {
            Some(c) => c,
            None => self.chars.next(),
        }
    }

    /// `\s*`
    fn eat_ws(&mut self) {
        while self.peek().is_some_and(crate::utils::is_js_whitespace) {
            self.bump();
        }
    }

    /// `\s+` — at least one whitespace char.
    fn eat_ws1(&mut self) -> bool {
        if self.peek().is_some_and(crate::utils::is_js_whitespace) {
            self.eat_ws();
            true
        } else {
            false
        }
    }

    /// Case-insensitive literal.
    fn lit(&mut self, s: &str) -> bool {
        let mut probe = Self {
            chars: self.chars.clone(),
            peeked: self.peeked,
        };
        for expected in s.chars() {
            if probe.bump().is_some_and(|c| c.to_ascii_lowercase() == expected) {
                continue;
            }
            return false;
        }
        *self = probe;
        true
    }

    /// NUMBER_PATTERN.
    fn number(&mut self) -> Option<f64> {
        let mut probe = Self {
            chars: self.chars.clone(),
            peeked: self.peeked,
        };
        let mut text = String::new();
        if probe.peek().is_some_and(|c| c == '+' || c == '-') {
            text.push(probe.bump().unwrap());
        }
        // (?:\d+(\.\d*)?|\.\d+)
        if probe.peek().is_some_and(|c| c.is_ascii_digit()) {
            while probe.peek().is_some_and(|c| c.is_ascii_digit()) {
                text.push(probe.bump().unwrap());
            }
            if probe.peek() == Some('.') {
                text.push(probe.bump().unwrap());
                while probe.peek().is_some_and(|c| c.is_ascii_digit()) {
                    text.push(probe.bump().unwrap());
                }
            }
        } else if probe.peek() == Some('.') {
            text.push(probe.bump().unwrap());
            if !probe.peek().is_some_and(|c| c.is_ascii_digit()) {
                return None;
            }
            while probe.peek().is_some_and(|c| c.is_ascii_digit()) {
                text.push(probe.bump().unwrap());
            }
        } else {
            return None;
        }
        // (?:e[+-]?\d+)? — case insensitive.
        let save = probe.clone_state();
        if probe.peek().is_some_and(|c| c == 'e' || c == 'E') {
            let mut exp = String::from("e");
            probe.bump();
            if probe.peek().is_some_and(|c| c == '+' || c == '-') {
                exp.push(probe.bump().unwrap());
            }
            if probe.peek().is_some_and(|c| c.is_ascii_digit()) {
                while probe.peek().is_some_and(|c| c.is_ascii_digit()) {
                    exp.push(probe.bump().unwrap());
                }
                text.push_str(&exp);
            } else {
                probe.restore(save);
            }
        }
        let value: f64 = text.parse().ok()?;
        *self = probe;
        Some(value)
    }

    fn clone_state(&self) -> (std::str::Chars<'a>, Option<Option<char>>) {
        (self.chars.clone(), self.peeked)
    }

    fn restore(&mut self, state: (std::str::Chars<'a>, Option<Option<char>>)) {
        self.chars = state.0;
        self.peeked = state.1;
    }

    fn at_end(&mut self) -> bool {
        self.peek().is_none()
    }
}

/// Port of `parseColor(value)` for string inputs.
pub fn parse_color(value: &str) -> Result<Color, String> {
    // /^#([\da-f]{3}|[\da-f]{6})$/i
    if let Some(hex) = value.strip_prefix('#') {
        let valid = matches!(hex.len(), 3 | 6) && hex.chars().all(|c| c.is_ascii_hexdigit());
        if valid {
            let digits: String = if hex.len() == 3 {
                hex.chars().flat_map(|d| [d, d]).collect()
            } else {
                hex.to_string()
            };
            let r = u32::from_str_radix(&digits[0..2], 16).unwrap();
            let g = u32::from_str_radix(&digits[2..4], 16).unwrap();
            let b = u32::from_str_radix(&digits[4..6], 16).unwrap();
            return Ok(Color::Rgb {
                r: r as f64,
                g: g as f64,
                b: b as f64,
            });
        }
    }

    // OKLCH_PATTERN: ^oklch\(\s*(NUM)(%)?\s+(NUM)\s+(NUM)(deg)?\s*\)$ (i)
    if let Some(color) = parse_oklch_pattern(value) {
        return color;
    }

    // OKHSL_PATTERN: ^okhsl\(\s*(NUM)(deg)?\s+(NUM)(%)?\s+(NUM)(%)?\s*\)$ (i)
    if let Some(color) = parse_okhsl_pattern(value) {
        return color;
    }

    Err(format!("Invalid color value: {value}"))
}

fn parse_oklch_pattern(value: &str) -> Option<Result<Color, String>> {
    let mut scan = ColorScan::new(value);
    if !scan.lit("oklch(") {
        return None;
    }
    scan.eat_ws();
    let lightness = scan.number()?;
    let percent = scan.lit("%");
    if !scan.eat_ws1() {
        return None;
    }
    let chroma = scan.number()?;
    if !scan.eat_ws1() {
        return None;
    }
    let hue = scan.number()?;
    let _deg = scan.lit("deg");
    scan.eat_ws();
    if !scan.lit(")") || !scan.at_end() {
        return None;
    }
    let l = lightness / if percent { 100.0 } else { 1.0 };
    Some(oklch_color(l, chroma, hue))
}

fn parse_okhsl_pattern(value: &str) -> Option<Result<Color, String>> {
    let mut scan = ColorScan::new(value);
    if !scan.lit("okhsl(") {
        return None;
    }
    scan.eat_ws();
    let hue = scan.number()?;
    let _deg = scan.lit("deg");
    if !scan.eat_ws1() {
        return None;
    }
    let saturation = scan.number()?;
    let s_percent = scan.lit("%");
    if !scan.eat_ws1() {
        return None;
    }
    let lightness = scan.number()?;
    let l_percent = scan.lit("%");
    scan.eat_ws();
    if !scan.lit(")") || !scan.at_end() {
        return None;
    }
    let s = saturation / if s_percent { 100.0 } else { 1.0 };
    let l = lightness / if l_percent { 100.0 } else { 1.0 };
    Some(okhsl_color(hue, s, l))
}

// --- indexed -> RGB tables -----------------------------------------------------

const BASIC_COLORS: [RgbColor; 16] = [
    RgbColor { r: 0.0, g: 0.0, b: 0.0 },
    RgbColor { r: 128.0, g: 0.0, b: 0.0 },
    RgbColor { r: 0.0, g: 128.0, b: 0.0 },
    RgbColor { r: 128.0, g: 128.0, b: 0.0 },
    RgbColor { r: 0.0, g: 0.0, b: 128.0 },
    RgbColor { r: 128.0, g: 0.0, b: 128.0 },
    RgbColor { r: 0.0, g: 128.0, b: 128.0 },
    RgbColor { r: 192.0, g: 192.0, b: 192.0 },
    RgbColor { r: 128.0, g: 128.0, b: 128.0 },
    RgbColor { r: 255.0, g: 0.0, b: 0.0 },
    RgbColor { r: 0.0, g: 255.0, b: 0.0 },
    RgbColor { r: 255.0, g: 255.0, b: 0.0 },
    RgbColor { r: 0.0, g: 0.0, b: 255.0 },
    RgbColor { r: 255.0, g: 0.0, b: 255.0 },
    RgbColor { r: 0.0, g: 255.0, b: 255.0 },
    RgbColor { r: 255.0, g: 255.0, b: 255.0 },
];
const CUBE_VALUES: [f64; 6] = [0.0, 95.0, 135.0, 175.0, 215.0, 255.0];
const GRAY_VALUES: [f64; 24] = {
    let mut values = [0.0f64; 24];
    let mut index = 0;
    while index < 24 {
        values[index] = 8.0 + index as f64 * 10.0;
        index += 1;
    }
    values
};

fn indexed_to_rgb(index: u8) -> RgbColor {
    if index < 16 {
        return BASIC_COLORS[index as usize];
    }
    if index < 232 {
        let cube_index = index as usize - 16;
        return RgbColor {
            r: CUBE_VALUES[cube_index / 36],
            g: CUBE_VALUES[(cube_index % 36) / 6],
            b: CUBE_VALUES[cube_index % 6],
        };
    }
    let gray = 8.0 + (index as f64 - 232.0) * 10.0;
    RgbColor {
        r: gray,
        g: gray,
        b: gray,
    }
}

fn is_in_srgb_gamut(linear: &[f64; 3]) -> bool {
    const EPSILON: f64 = 1e-7;
    linear
        .iter()
        .all(|&channel| channel >= -EPSILON && channel <= 1.0 + EPSILON)
}

fn oklch_to_rgb(OklchChannels { l, c, h }: OklchChannels) -> RgbColor {
    // Gamut mapping keeps the hue fixed, so its direction is computed once
    // and scaled by chroma.
    let radians = (h * std::f64::consts::PI) / 180.0;
    let cos = crate::fdlibm::cos(radians);
    let sin = crate::fdlibm::sin(radians);
    let at_chroma = |chroma: f64| oklab_to_linear_srgb([l, chroma * cos, chroma * sin]);

    let direct = at_chroma(c);
    if is_in_srgb_gamut(&direct) {
        return linear_srgb_to_rgb(direct);
    }

    // Reduce chroma until the color fits. The achromatic color is always in
    // gamut, so it is the fallback when no bisection step fits, e.g.
    // `oklch(100% 0.3 150)` must map to white.
    let mut linear = at_chroma(0.0);
    let mut low = 0.0f64;
    let mut high = c;
    for _ in 0..20 {
        let chroma = (low + high) / 2.0;
        let candidate = at_chroma(chroma);
        if is_in_srgb_gamut(&candidate) {
            low = chroma;
            linear = candidate;
        } else {
            high = chroma;
        }
    }
    linear_srgb_to_rgb(linear)
}

pub fn color_to_rgb(color: Color) -> RgbColor {
    match color {
        Color::Indexed { index } => indexed_to_rgb(index),
        Color::Rgb { r, g, b } => RgbColor { r, g, b },
        Color::Oklch { l, c, h } => oklch_to_rgb(OklchChannels { l, c, h }),
    }
}

pub fn color_to_oklch(color: Color) -> OklchChannels {
    if let Color::Oklch { l, c, h } = color {
        return OklchChannels { l, c, h };
    }
    let [l, a, b] = rgb_to_oklab(color_to_rgb(color));
    OklchChannels {
        l,
        c: crate::fdlibm::hypot(a, b),
        h: ((crate::fdlibm::atan2(b, a) * 180.0) / std::f64::consts::PI + 360.0) % 360.0,
    }
}

pub fn color_to_hex(color: Color) -> String {
    let RgbColor { r, g, b } = color_to_rgb(color);
    let channel =
        |value: f64| format!("{:02x}", js_round(value).max(0.0) as i64);
    format!("#{}{}{}", channel(r), channel(g), channel(b))
}

/// Port of `mixColors`.
pub fn mix_colors(
    first: Color,
    second: Color,
    amount: f64,
    space: ColorMixSpace,
) -> Result<Color, String> {
    if !amount.is_finite() {
        return Err("amount must be finite".to_string());
    }
    if !(0.0..=1.0).contains(&amount) {
        return Err(format!(
            "amount must be between 0 and 1: {}",
            js_number_to_string(amount)
        ));
    }

    if space == ColorMixSpace::Srgb {
        let a = color_to_rgb(first);
        let b = color_to_rgb(second);
        return rgb_color(
            a.r + (b.r - a.r) * amount,
            a.g + (b.g - a.g) * amount,
            a.b + (b.b - a.b) * amount,
        );
    }

    let a = color_to_oklch(first);
    let b = color_to_oklch(second);
    let first_hue = if a.c < 1e-7 { b.h } else { a.h };
    let second_hue = if b.c < 1e-7 { first_hue } else { b.h };
    let hue_delta = ((second_hue - first_hue + 540.0) % 360.0) - 180.0;
    oklch_color(
        a.l + (b.l - a.l) * amount,
        a.c + (b.c - a.c) * amount,
        first_hue + hue_delta * amount,
    )
}

fn find_closest(values: &[f64], target: f64) -> usize {
    let mut closest_index = 0;
    let mut closest_distance = f64::INFINITY;
    for (index, &value) in values.iter().enumerate() {
        let distance = (target - value).abs();
        if distance < closest_distance {
            closest_index = index;
            closest_distance = distance;
        }
    }
    closest_index
}

fn color_distance(first: RgbColor, second: RgbColor) -> f64 {
    let dr = first.r - second.r;
    let dg = first.g - second.g;
    let db = first.b - second.b;
    dr * dr * 0.299 + dg * dg * 0.587 + db * db * 0.114
}

fn rgb_to_ansi256(color: RgbColor) -> u32 {
    let r_index = find_closest(&CUBE_VALUES, color.r);
    let g_index = find_closest(&CUBE_VALUES, color.g);
    let b_index = find_closest(&CUBE_VALUES, color.b);
    let cube_color = RgbColor {
        r: CUBE_VALUES[r_index],
        g: CUBE_VALUES[g_index],
        b: CUBE_VALUES[b_index],
    };
    let cube_index = 16 + 36 * r_index + 6 * g_index + b_index;

    let gray = js_round(0.299 * color.r + 0.587 * color.g + 0.114 * color.b);
    let gray_offset = find_closest(&GRAY_VALUES, gray);
    let gray_value = GRAY_VALUES[gray_offset];
    let spread = color.r.max(color.g).max(color.b) - color.r.min(color.g).min(color.b);
    if spread < 10.0
        && color_distance(
            color,
            RgbColor {
                r: gray_value,
                g: gray_value,
                b: gray_value,
            },
        ) < color_distance(color, cube_color)
    {
        return 232 + gray_offset as u32;
    }
    cube_index as u32
}

fn color_ansi(color: Color, mode: TerminalColorMode, background: bool) -> String {
    if let Color::Indexed { index } = color {
        return format!("\x1b[{};5;{}m", if background { 48 } else { 38 }, index);
    }

    let rgb = color_to_rgb(color);
    if mode == TerminalColorMode::TrueColor {
        return format!(
            "\x1b[{};2;{};{};{}m",
            if background { 48 } else { 38 },
            js_round(rgb.r) as i64,
            js_round(rgb.g) as i64,
            js_round(rgb.b) as i64
        );
    }
    format!(
        "\x1b[{};5;{}m",
        if background { 48 } else { 38 },
        rgb_to_ansi256(rgb)
    )
}

pub fn foreground_ansi(color: Color, mode: TerminalColorMode) -> String {
    color_ansi(color, mode, false)
}

pub fn background_ansi(color: Color, mode: TerminalColorMode) -> String {
    color_ansi(color, mode, true)
}

pub fn style_text(text: &str, options: &TextStyle, mode: TerminalColorMode) -> String {
    let fg_ansi = options.fg.map(|c| foreground_ansi(c, mode));
    let bg_ansi = options.bg.map(|c| background_ansi(c, mode));
    style_text_with_ansi(text, fg_ansi.as_deref(), bg_ansi.as_deref(), &options.attributes())
}

/// Like [`style_text`], but with precomputed color escape sequences, e.g.
/// cached theme colors. Colors in `options` are ignored (the TS signature
/// takes only attributes here).
pub fn style_text_with_ansi(
    text: &str,
    fg_ansi: Option<&str>,
    bg_ansi: Option<&str>,
    options: &TextAttributes,
) -> String {
    // Resets are prepended so they close in reverse order of the opening
    // sequences.
    let mut prefix = String::new();
    let mut suffix = String::new();
    if let Some(fg) = fg_ansi {
        prefix.push_str(fg);
        suffix = "\x1b[39m".to_string();
    }
    if let Some(bg) = bg_ansi {
        prefix.push_str(bg);
        suffix = format!("\x1b[49m{suffix}");
    }
    if options.bold {
        prefix.push_str("\x1b[1m");
    }
    if options.dim {
        prefix.push_str("\x1b[2m");
    }
    if options.bold || options.dim {
        suffix = format!("\x1b[22m{suffix}");
    }
    if options.italic {
        prefix.push_str("\x1b[3m");
        suffix = format!("\x1b[23m{suffix}");
    }
    if options.underline {
        prefix.push_str("\x1b[4m");
        suffix = format!("\x1b[24m{suffix}");
    }
    if options.inverse {
        prefix.push_str("\x1b[7m");
        suffix = format!("\x1b[27m{suffix}");
    }
    if options.strikethrough {
        prefix.push_str("\x1b[9m");
        suffix = format!("\x1b[29m{suffix}");
    }
    format!("{prefix}{text}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_and_oklch_colors_and_rejects_everything_else() {
        assert_eq!(
            parse_color("#abc").unwrap(),
            Color::Rgb {
                r: 170.0,
                g: 187.0,
                b: 204.0
            }
        );
        assert_eq!(
            parse_color("oklch(62% 0.1 200)").unwrap(),
            Color::Oklch {
                l: 0.62,
                c: 0.1,
                h: 200.0
            }
        );
        assert!(parse_color("").unwrap_err().contains("Invalid color value"));
        assert!(parse_color("red").unwrap_err().contains("Invalid color value"));
        assert_eq!(parse_color("#ABCDEF").unwrap(), parse_color("#abcdef").unwrap());
    }

    #[test]
    fn gamut_maps_oklch_to_srgb_including_lightness_limits() {
        assert_eq!(
            color_to_rgb(Color::Oklch {
                l: 0.627955,
                c: 0.257683,
                h: 29.2339
            }),
            RgbColor {
                r: 255.0,
                g: 0.0,
                b: 0.0
            }
        );
        assert_eq!(
            color_to_rgb(Color::Oklch {
                l: 1.0,
                c: 0.3,
                h: 150.0
            }),
            RgbColor {
                r: 255.0,
                g: 255.0,
                b: 255.0
            }
        );
        assert_eq!(
            color_to_rgb(Color::Oklch {
                l: 0.0,
                c: 0.3,
                h: 150.0
            }),
            RgbColor {
                r: 0.0,
                g: 0.0,
                b: 0.0
            }
        );
    }

    #[test]
    fn parses_okhsl_colors_and_round_trips_them() {
        // Full saturation at the red cusp is pure sRGB red.
        assert_eq!(
            parse_color("okhsl(29.23 100% 56.8%)").unwrap(),
            rgb_color(255.0, 0.0, 0.0).unwrap()
        );
        assert_eq!(
            parse_color("OKHSL(250deg 60% 55%)").unwrap(),
            okhsl_color(250.0, 0.6, 0.55).unwrap()
        );
        assert!(parse_color("okhsl(250 160% 55%)")
            .unwrap_err()
            .contains("s must be between 0 and 1"));
        for hex in ["#4f8eb3", "#20242a", "#f8f9fa"] {
            let color = parse_color(hex).unwrap();
            let OkhslChannels { h, s, l } = color_to_okhsl(color);
            assert_eq!(color_to_hex(okhsl_color(h, s, l).unwrap()), hex);
        }
    }

    #[test]
    fn styles_text_and_closes_sequences_in_reverse_order() {
        let styled = style_text(
            "Ready",
            &TextStyle {
                fg: Some(Color::Rgb {
                    r: 18.0,
                    g: 52.0,
                    b: 86.0,
                }),
                bg: Some(Color::Indexed { index: 9 }),
                bold: true,
                italic: true,
                ..Default::default()
            },
            TerminalColorMode::TrueColor,
        );
        assert_eq!(
            styled,
            "\x1b[38;2;18;52;86m\x1b[48;5;9m\x1b[1m\x1b[3mReady\x1b[23m\x1b[22m\x1b[49m\x1b[39m"
        );
        let truecolor = style_text(
            "Ready",
            &TextStyle {
                fg: Some(Color::Rgb {
                    r: 18.0,
                    g: 52.0,
                    b: 86.0,
                }),
                ..Default::default()
            },
            TerminalColorMode::Color256,
        );
        assert!(truecolor.starts_with("\x1b[38;5;") && truecolor.ends_with("mReady\x1b[39m"));
    }

    #[test]
    fn indexed_color_validates_range() {
        assert!(indexed_color(0.0).is_ok());
        assert!(indexed_color(255.0).is_ok());
        assert!(indexed_color(-1.0).is_err());
        assert!(indexed_color(256.0).is_err());
        assert!(indexed_color(1.5).is_err());
    }

    #[test]
    fn mixes_colors_in_oklch_and_srgb() {
        let black = Color::Rgb {
            r: 0.0,
            g: 0.0,
            b: 0.0,
        };
        let white = Color::Rgb {
            r: 255.0,
            g: 255.0,
            b: 255.0,
        };
        let mid_srgb = mix_colors(black, white, 0.5, ColorMixSpace::Srgb).unwrap();
        assert_eq!(
            mid_srgb,
            Color::Rgb {
                r: 127.5,
                g: 127.5,
                b: 127.5
            }
        );
        let mid_oklch = mix_colors(black, white, 0.5, ColorMixSpace::Oklch).unwrap();
        // Default space is oklch; the mix stays gray (c == 0).
        assert!(matches!(mid_oklch, Color::Oklch { c, .. } if c < 1e-9));
        assert!(mix_colors(black, white, 1.5, ColorMixSpace::Oklch).is_err());
    }

    #[test]
    fn parses_numbers_in_color_patterns() {
        assert!(parse_color("oklch(.5 .1 30)").is_ok());
        assert!(parse_color("oklch(+.5e0 1e-1 30)").is_ok());
        assert!(parse_color("oklch(0.5 0.1 30deg)").is_ok());
        assert!(parse_color("oklch(50% 0.1 30) ").is_err());
        assert!(parse_color("okhsl(29.23 100% 56.8%)x").is_err());
        assert!(parse_color("oklch(0.5 0.1)").is_err());
    }
}
