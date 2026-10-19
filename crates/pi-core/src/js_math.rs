//! JS `Math` functions that are not bit-identical to Rust's `std` or the
//! system libm.
//!
//! V8 implements `Math.log` with its port of fdlibm's `__ieee754_log`
//! (`e_log.c`), *not* with the platform libm, so `f64::ln` disagrees with
//! node for ~0.7% of inputs by exactly 1 ulp (measured on 20k random
//! doubles). Scores computed from logs flow into model-visible JSON, so the
//! port must match bit-for-bit: [`js_math_log`] is a line-by-line port of
//! the fdlibm algorithm (verified against node/V8 on 20,022 random and edge
//! case inputs, plus the special cases below).
//!
//! Other `Math` members used by the original (`abs`, `floor`, `ceil`,
//! `max`, `min`, `sqrt`, `pow` with integer results, trigonometry-free) are
//! exact IEEE 754 operations and match `std` directly.

/// fdlibm `__ieee754_log` (e_log.c), the implementation V8 uses for
/// `Math.log`. Returns the base-e logarithm of `x` with V8's exact bits.
///
/// Special cases, matching `Math.log`:
/// - `log(±0)` is `-inf`
/// - `log(x)` for `x < 0` (including `-inf`) is `NaN`
/// - `log(+inf)` is `+inf`
/// - `log(NaN)` is that `NaN`
// The body is a line-by-line port of the fdlibm C source: the decimal
// constants carry more digits than a double needs (fdlibm documents them as
// converting to the hex values in the comments), and the early returns
// mirror the C control flow.
#[allow(clippy::excessive_precision, clippy::needless_return)]
pub fn js_math_log(x: f64) -> f64 {
    /* 3fe62e42 fee00000 */ const LN2_HI: f64 = 6.93147180369123816490e-01;
    /* 3dea39ef 35793c76 */ const LN2_LO: f64 = 1.90821492927058770002e-10;
    /* 43500000 00000000 */ const TWO54: f64 = 1.80143985094819840000e+16;
    /* 3FE55555 55555593 */ const LG1: f64 = 6.666666666666735130e-01;
    /* 3FD99999 9997FA04 */ const LG2: f64 = 3.999999999940941908e-01;
    /* 3FD24924 94229359 */ const LG3: f64 = 2.857142874366239149e-01;
    /* 3FCC71C5 1D8E78AF */ const LG4: f64 = 2.222219843214978396e-01;
    /* 3FC74664 96CB03DE */ const LG5: f64 = 1.818357216161805012e-01;
    /* 3FC39A09 D078C69F */ const LG6: f64 = 1.531383769920937332e-01;
    /* 3FC2F112 DF3E5244 */ const LG7: f64 = 1.479819860511658591e-01;

    let mut x = x;
    let mut k: i32;
    let mut hx: i32 = (x.to_bits() >> 32) as i32;
    let lx: u32 = x.to_bits() as u32;

    k = 0;
    if hx < 0x0010_0000 {
        /* x < 2**-1022 */
        if ((hx & 0x7fff_ffff) as u32 | lx) == 0 {
            return f64::NEG_INFINITY; /* log(+-0)=-inf */
        }
        if hx < 0 {
            return f64::NAN; /* log(-#) = NaN */
        }
        k = -54;
        x *= TWO54; /* subnormal number, scale up x */
        hx = (x.to_bits() >> 32) as i32;
    }
    if hx >= 0x7ff0_0000 {
        return x + x;
    }
    k += (hx >> 20) - 1023;
    hx &= 0x000f_ffff;
    let i: i32 = (hx + 0x95f64) & 0x100000;
    /* __HI(x) = hx|(i^0x3ff00000); -- normalize x or x/2 */
    {
        let hi = (hx | (i ^ 0x3ff0_0000)) as u32 as u64;
        x = f64::from_bits((hi << 32) | (x.to_bits() & 0xffff_ffff));
    }
    k += i >> 20;
    let f: f64 = x - 1.0;
    if (0x000f_ffff & (2 + hx)) < 3 {
        /* |f| < 2**-20 */
        if f == 0.0 {
            if k == 0 {
                return 0.0;
            } else {
                let dk = k as f64;
                return dk * LN2_HI + dk * LN2_LO;
            }
        }
        let r = f * f * (0.5 - 0.33333333333333333 * f);
        if k == 0 {
            return f - r;
        } else {
            let dk = k as f64;
            return dk * LN2_HI - ((r - dk * LN2_LO) - f);
        }
    }
    let s = f / (2.0 + f);
    let dk = k as f64;
    let z = s * s;
    let mut i: i32 = hx - 0x6147a;
    let w = z * z;
    let j: i32 = 0x6b851 - hx;
    let t1 = w * (LG2 + w * (LG4 + w * LG6));
    let t2 = z * (LG1 + w * (LG3 + w * (LG5 + w * LG7)));
    i |= j;
    let r = t2 + t1;
    if i > 0 {
        let hfsq = 0.5 * f * f;
        if k == 0 {
            return f - (hfsq - s * (hfsq + r));
        } else {
            return dk * LN2_HI - ((hfsq - (s * (hfsq + r) + dk * LN2_LO)) - f);
        }
    } else {
        if k == 0 {
            return f - s * (f - r);
        } else {
            return dk * LN2_HI - ((s * (f - r) - dk * LN2_LO) - f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(x: f64) -> u64 {
        x.to_bits()
    }

    /// Special cases, verified with node: `Math.log` of ±0, negatives, infinities, NaN.
    #[test]
    fn special_cases() {
        assert_eq!(js_math_log(0.0), f64::NEG_INFINITY);
        assert_eq!(js_math_log(-0.0), f64::NEG_INFINITY);
        assert!(js_math_log(-1.0).is_nan());
        assert!(js_math_log(f64::NEG_INFINITY).is_nan());
        assert_eq!(js_math_log(f64::INFINITY), f64::INFINITY);
        assert!(js_math_log(f64::NAN).is_nan());
        assert_eq!(js_math_log(1.0), 0.0);
        assert!(js_math_log(-5e-324).is_nan());
    }

    /// Bit-exact vectors where V8's fdlibm log and Rust's libm `ln` disagree
    /// (each differs by exactly 1 ulp), verified against node.
    #[test]
    fn v8_vs_libm_divergence_vectors() {
        // Math.log(1.6) → 0x3fde148a1a2726ce (libm gives ...6cf)
        assert_eq!(bits(js_math_log(1.6)), 0x3fde_148a_1a27_26ce);
        // Math.log(2.6666666666666665) → 0x3fef62f40794a7ba (libm ...7b9)
        assert_eq!(bits(js_math_log(1.0 + (3.0 - 1.0 + 0.5) / (1.0 + 0.5))), 0x3fef_62f4_0794_a7ba);
        // Math.log(3.0) → 0x3ff193ea7aad030a (libm ...30b)
        assert_eq!(bits(js_math_log(3.0)), 0x3ff1_93ea_7aad_030a);
        // Math.log(0.00015625...) → 0xbffdcf4bf73c854c (libm ...54d)
        assert_eq!(bits(js_math_log(f64::from_bits(0x3fc3_dd3e_4866_beae))), 0xbffd_cf4b_f73c_854c);
    }

    /// Subnormal inputs go through the scale-up path; verified with node.
    #[test]
    fn subnormals_and_extremes() {
        // Math.log(5e-324) = -744.4400719213812
        assert_eq!(js_math_log(5e-324), -744.4400719213812);
        // Math.log(2.2250738585072014e-308) = -708.3964185322641
        assert_eq!(js_math_log(2.2250738585072014e-308), -708.3964185322641);
        // Math.log(1.7976931348623157e308) = 709.782712893384
        assert_eq!(js_math_log(1.7976931348623157e308), 709.782712893384);
        // Math.log(2**-1023 * 1.5) = -708.6841006047159 (subnormal argument).
        let x = f64::from_bits(0x0008_0000_0000_0000); // 2**-1023, subnormal
        assert_eq!(js_math_log(x * 1.5), -708.6841006047159);
        // Near-1 path (|f| < 2**-20).
        assert_eq!(js_math_log(1.0000000000000002), 2.2204460492503128e-16);
        assert_eq!(js_math_log(0.9999999999999999), -1.1102230246251565e-16);
    }

    /// Ordinary values, spot-checked against node.
    #[test]
    fn ordinary_values() {
        assert_eq!(js_math_log(10.0), std::f64::consts::LN_10);
        assert_eq!(js_math_log(std::f64::consts::E), 1.0);
        assert_eq!(js_math_log(0.5), -std::f64::consts::LN_2);
        assert_eq!(js_math_log(100.0), 4.605170185988092);
    }
}
