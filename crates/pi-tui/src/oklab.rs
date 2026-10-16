//! Oklab and OKHSL <-> sRGB conversion — port of `oklab.ts`.
//!
//! `colors.rs` builds its OKLCH, OKHSL, and color mixing on it.
//!
//! Oklab and OKHSL are Björn Ottosson's color spaces; OKHSL's saturation is
//! relative to the sRGB gamut at each hue and lightness. This is a port of
//! his reference implementation (<https://bottosson.github.io/posts/colorpicker/>),
//! Copyright (c) 2021 Björn Ottosson, used under the MIT license:
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy of this software and
//! associated documentation files (the "Software"), to deal in the Software without restriction, including
//! without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to
//! the following conditions: The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software. THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY
//! KIND, EXPRESS OR IMPLIED.

use crate::terminal_colors::RgbColor;

pub type Vector = [f64; 3];
type Matrix = [Vector; 3];

fn multiply(m: &Matrix, v: Vector) -> Vector {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

// ============================================================================
// OKHSL <-> sRGB
// ============================================================================

const LINEAR_SRGB_TO_LMS: Matrix = [
    [0.4122214694707629, 0.5363325372617349, 0.0514459932675022],
    [0.2119034958178251, 0.6806995506452344, 0.1073969535369405],
    [0.0883024591900564, 0.2817188391361215, 0.6299787016738222],
];
const LMS_TO_LAB: Matrix = [
    [0.210454268309314, 0.793617774702305, -0.0040720430116193],
    [1.9779985324311684, -2.42859224204858, 0.450593709617411],
    [0.0259040424655478, 0.7827717124575296, -0.8086757549230774],
];
const LAB_TO_LMS: Matrix = [
    [1.0, 0.3963377773761749, 0.2158037573099136],
    [1.0, -0.1055613458156586, -0.0638541728258133],
    [1.0, -0.0894841775298119, -1.2914855480194092],
];
const LMS_TO_LINEAR_SRGB: Matrix = [
    [4.0767416360759583, -3.3077115392580629, 0.2309699031821043],
    [-1.2684379732850315, 2.6097573492876882, -0.341319376002657],
    [-0.0041960761386756, -0.7034186179359362, 1.7076146940746117],
];

/// Per sRGB channel (red, green, blue): the (a, b) half-plane where that
/// channel clips first, and the polynomial approximating the maximum
/// saturation there.
const SATURATION_FIT: [([f64; 2], [f64; 5]); 3] = [
    (
        [-1.8817031, -0.80936501],
        [1.19086277, 1.76576728, 0.59662641, 0.75515197, 0.56771245],
    ),
    (
        [1.8144408, -1.19445267],
        [0.73956515, -0.45954404, 0.08285427, 0.12541073, -0.14503204],
    ),
    (
        [0.13110758, 1.81333971],
        [1.35733652, -0.00915799, -1.1513021, -0.50559606, 0.00692167],
    ),
];
const K1: f64 = 0.206;
const K2: f64 = 0.03;
const K3: f64 = (1.0 + K1) / (1.0 + K2);

/// Oklab lightness to OKHSL lightness.
pub fn oklab_to_okhsl_lightness(x: f64) -> f64 {
    0.5 * (K3 * x - K1 + ((K3 * x - K1) * (K3 * x - K1) + 4.0 * K2 * K3 * x).sqrt())
}

/// OKHSL lightness to Oklab lightness.
fn okhsl_to_oklab_lightness(x: f64) -> f64 {
    (x * x + K1 * x) / (K3 * (x + K2))
}

/// sRGB transfer function: linear to encoded channel, both 0-1.
fn linear_to_srgb(value: f64) -> f64 {
    if value > 0.0031308 {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    } else {
        12.92 * value
    }
}

/// Inverse sRGB transfer function: encoded to linear channel, both 0-1.
fn srgb_to_linear(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// JS `Math.round`: nearest integer, ties toward +infinity. Guarded against
/// the `x + 0.5` double-rounding edge (e.g. 0.49999999999999994).
pub fn js_round(x: f64) -> f64 {
    if x >= 4503599627370496.0 || x <= -4503599627370496.0 {
        return x; // |x| >= 2^52: already integral
    }
    let rounded = (x + 0.5).floor();
    if rounded - x > 0.5 {
        rounded - 1.0
    } else {
        rounded
    }
}

/// Oklab [L, a, b] to linear sRGB [r, g, b] (0-1, may leave the gamut).
pub fn oklab_to_linear_srgb(lab: Vector) -> Vector {
    let lms = multiply(&LAB_TO_LMS, lab).map(|v| v * v * v);
    multiply(&LMS_TO_LINEAR_SRGB, lms)
}

/// Linear sRGB [r, g, b] (0-1) to Oklab [L, a, b].
fn linear_srgb_to_oklab(rgb: Vector) -> Vector {
    let lms = multiply(&LINEAR_SRGB_TO_LMS, rgb).map(crate::fdlibm::cbrt);
    multiply(&LMS_TO_LAB, lms)
}

/// sRGB channels (0-255) to Oklab [L, a, b].
pub fn rgb_to_oklab(rgb: RgbColor) -> Vector {
    linear_srgb_to_oklab([
        srgb_to_linear(rgb.r / 255.0),
        srgb_to_linear(rgb.g / 255.0),
        srgb_to_linear(rgb.b / 255.0),
    ])
}

/// Linear sRGB [r, g, b] (0-1) to sRGB channels (0-255, rounded), clipping
/// out-of-gamut channels.
pub fn linear_srgb_to_rgb(linear: Vector) -> RgbColor {
    let encoded = linear.map(|v| js_round(linear_to_srgb(v).clamp(0.0, 1.0) * 255.0));
    RgbColor {
        r: encoded[0],
        g: encoded[1],
        b: encoded[2],
    }
}

/// Rate of change of each cube-root LMS component along a chroma direction
/// (a, b).
fn lms_slopes(a: f64, b: f64) -> Vector {
    [
        LAB_TO_LMS[0][1] * a + LAB_TO_LMS[0][2] * b,
        LAB_TO_LMS[1][1] * a + LAB_TO_LMS[1][2] * b,
        LAB_TO_LMS[2][1] * a + LAB_TO_LMS[2][2] * b,
    ]
}

/// Largest saturation (C/L) inside sRGB for hue (a, b): polynomial fit plus
/// one Halley step.
fn max_saturation(a: f64, b: f64) -> f64 {
    let channel = (0..3)
        .find(|&index| {
            index == 2 || SATURATION_FIT[index].0[0] * a + SATURATION_FIT[index].0[1] * b > 1.0
        })
        .expect("channel 2 always matches");
    let fit = SATURATION_FIT[channel].1;
    let weights = LMS_TO_LINEAR_SRGB[channel];
    let saturation = fit[0] + fit[1] * a + fit[2] * b + fit[3] * a * a + fit[4] * a * b;

    let slopes = lms_slopes(a, b);
    let base = slopes.map(|k| 1.0 + saturation * k);
    let dot = |values: Vector| weights[0] * values[0] + weights[1] * values[1] + weights[2] * values[2];
    let f = dot(base.map(|v| v * v * v));
    let f1 = dot([
        3.0 * slopes[0] * base[0] * base[0],
        3.0 * slopes[1] * base[1] * base[1],
        3.0 * slopes[2] * base[2] * base[2],
    ]);
    let f2 = dot([
        6.0 * slopes[0] * slopes[0] * base[0],
        6.0 * slopes[1] * slopes[1] * base[1],
        6.0 * slopes[2] * slopes[2] * base[2],
    ]);
    saturation - (f * f1) / (f1 * f1 - 0.5 * f * f2)
}

/// Oklab lightness and chroma of the most saturated sRGB color of hue (a, b).
fn cusp(a: f64, b: f64) -> (f64, f64) {
    let saturation = max_saturation(a, b);
    let linear = oklab_to_linear_srgb([1.0, saturation * a, saturation * b]);
    let max = linear[0].max(linear[1]).max(linear[2]);
    let lightness = crate::fdlibm::cbrt(1.0 / max);
    (lightness, lightness * saturation)
}

/// Chroma where the constant-lightness line at `lightness` leaves the sRGB
/// gamut.
fn max_chroma(a: f64, b: f64, lightness: f64, (cusp_l, cusp_c): (f64, f64)) -> f64 {
    if lightness <= cusp_l {
        return (cusp_c * lightness) / cusp_l;
    }
    // Upper half: triangle edge, then one Halley step against each channel
    // reaching 1.
    let t = (cusp_c * (lightness - 1.0)) / (cusp_l - 1.0);
    let slopes = lms_slopes(a, b);
    let lms = slopes.map(|k| lightness + t * k);
    let cubes = lms.map(|v| v * v * v);
    let first = [
        3.0 * slopes[0] * lms[0] * lms[0],
        3.0 * slopes[1] * lms[1] * lms[1],
        3.0 * slopes[2] * lms[2] * lms[2],
    ];
    let second = [
        6.0 * slopes[0] * slopes[0] * lms[0],
        6.0 * slopes[1] * slopes[1] * lms[1],
        6.0 * slopes[2] * slopes[2] * lms[2],
    ];
    let dot = |row: Vector, values: Vector| {
        row[0] * values[0] + row[1] * values[1] + row[2] * values[2]
    };
    let steps = LMS_TO_LINEAR_SRGB.map(|row| {
        let f = dot(row, cubes) - 1.0;
        let f1 = dot(row, first);
        let f2 = dot(row, second);
        let u = f1 / (f1 * f1 - 0.5 * f * f2);
        if u >= 0.0 {
            -f * u
        } else {
            f64::MAX
        }
    });
    t + steps[0].min(steps[1]).min(steps[2])
}

/// OKHSL's chroma reference points at lightness L and hue (a, b):
/// [c0, cMid, cMax].
fn chroma_stops(l: f64, a: f64, b: f64) -> (f64, f64, f64) {
    let peak = cusp(a, b);
    let c_max = max_chroma(a, b, l, peak);
    let k = c_max / (l * (peak.1 / peak.0)).min((1.0 - l) * (peak.1 / (1.0 - peak.0)));
    let mid_s = 0.11516993
        + 1.0 / (7.4477897
            + 4.1590124 * b
            + a * (-2.19557347
                + 1.75198401 * b
                + a * (-2.13704948 - 10.02301043 * b + a * (-4.24894561 + 5.38770819 * b + 4.69891013 * a))));
    let mid_t = 0.11239642
        + 1.0 / (1.6132032
            - 0.68124379 * b
            + a * (0.40370612
                + 0.90148123 * b
                + a * (-0.27087943 + 0.6122399 * b + a * (0.00299215 - 0.45399568 * b - 0.14661872 * a))));
    let c_mid = 0.9
        * k
        * (1.0 / (1.0 / (l * mid_s).powi(4) + 1.0 / ((1.0 - l) * mid_t).powi(4)))
            .sqrt()
            .sqrt();
    let c0 = (1.0 / (1.0 / (l * 0.4).powi(2) + 1.0 / ((1.0 - l) * 0.8).powi(2))).sqrt();
    (c0, c_mid, c_max)
}

/// Convert OKHSL to sRGB channels (0-255, rounded), clipping out-of-gamut
/// channels.
///
/// * `hue` — hue in degrees
/// * `saturation` — saturation, 0-1
/// * `lightness` — lightness, 0-1
pub fn okhsl_to_rgb(hue: f64, saturation: f64, lightness: f64) -> RgbColor {
    let l = okhsl_to_oklab_lightness(lightness);
    let mut lab = [l, 0.0, 0.0];
    if l > 0.0 && l < 1.0 && saturation > 0.0 {
        let angle = (2.0 * std::f64::consts::PI * (((hue % 360.0) + 360.0) % 360.0)) / 360.0;
        let a = crate::fdlibm::cos(angle);
        let b = crate::fdlibm::sin(angle);
        let (c0, c_mid, c_max) = chroma_stops(l, a, b);
        // Chroma rises from 0 through cMid at s = 0.8 to cMax at s = 1.
        let chroma;
        if saturation < 0.8 {
            let t = 1.25 * saturation;
            let k1 = 0.8 * c0;
            chroma = (t * k1) / (1.0 - (1.0 - k1 / c_mid) * t);
        } else {
            let t = 5.0 * (saturation - 0.8);
            let k1 = 0.2 * (c_mid * c_mid) * (1.25 * 1.25) / c0;
            chroma = c_mid + (t * k1) / (1.0 - (1.0 - k1 / (c_max - c_mid)) * t);
        }
        lab = [l, chroma * a, chroma * b];
    }
    linear_srgb_to_rgb(oklab_to_linear_srgb(lab))
}

/// Convert sRGB channels (0-255) to OKHSL.
///
/// Returns hue `h` in degrees (0 for grays), saturation `s` and lightness
/// `l` 0-1.
pub fn rgb_to_okhsl(rgb: RgbColor) -> (f64, f64, f64) {
    let [l, lab_a, lab_b] = rgb_to_oklab(rgb);
    let chroma = crate::fdlibm::hypot(lab_a, lab_b);
    let lightness = oklab_to_okhsl_lightness(l);
    if chroma < 1e-9 || lightness <= 0.0 || lightness >= 1.0 {
        return (0.0, 0.0, lightness);
    }

    let hue = ((crate::fdlibm::atan2(lab_b, lab_a) * 180.0) / std::f64::consts::PI + 360.0) % 360.0;
    let (c0, c_mid, c_max) = chroma_stops(l, lab_a / chroma, lab_b / chroma);
    let saturation;
    if chroma < c_mid {
        let k1 = 0.8 * c0;
        saturation = 0.8 * (chroma / (k1 + (1.0 - k1 / c_mid) * chroma));
    } else {
        let k1 = 0.2 * (c_mid * c_mid) * (1.25 * 1.25) / c0;
        let offset = chroma - c_mid;
        saturation = 0.8 + 0.2 * (offset / (k1 + (1.0 - k1 / (c_max - c_mid)) * offset));
    }
    (hue, saturation.clamp(0.0, 1.0), lightness)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(r: f64, g: f64, b: f64) -> RgbColor {
        RgbColor { r, g, b }
    }

    #[test]
    fn full_saturation_at_the_red_cusp_is_pure_red() {
        assert_eq!(okhsl_to_rgb(29.23, 1.0, 0.568), rgb(255.0, 0.0, 0.0));
    }

    #[test]
    fn round_trips_hex_colors_through_okhsl() {
        for (r, g, b) in [(79.0, 142.0, 179.0), (32.0, 36.0, 42.0), (248.0, 249.0, 250.0)] {
            let (h, s, l) = rgb_to_okhsl(rgb(r, g, b));
            assert_eq!(okhsl_to_rgb(h, s, l), rgb(r, g, b));
        }
    }

    #[test]
    fn gray_maps_to_zero_hue_and_saturation() {
        let (h, s, _l) = rgb_to_okhsl(rgb(128.0, 128.0, 128.0));
        assert_eq!(h, 0.0);
        assert_eq!(s, 0.0);
    }

    #[test]
    fn black_and_white_okhsl_lightness() {
        assert_eq!(rgb_to_okhsl(rgb(0.0, 0.0, 0.0)), (0.0, 0.0, 0.0));
        let (_, _, white_l) = rgb_to_okhsl(rgb(255.0, 255.0, 255.0));
        assert!((white_l - 1.0).abs() < 1e-12);
    }
}
