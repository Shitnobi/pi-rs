//! Bit-exact ports of the `Math.*` functions V8 uses in Node, so the Rust TUI
//! produces byte-identical floating-point results to the TypeScript original.
//!
//! `Math.atan2`, `Math.sin`, `Math.cos` and `Math.cbrt` in V8 come from
//! `src/base/ieee754.cc`, Google's port of fdlibm (http://www.netlib.org/fdlibm).
//! `Math.hypot` comes from V8's Torque builtin (`src/builtins/math.tq`,
//! `FastMathHypot`). These were ported from the V8 sources at tag 13.6.233,
//! the V8 version shipped in Node 24 (13.6.233.17-node.57), which is the
//! reference runtime for the parity suite.
//!
//! glibc's libm (used by Rust's std) implements these functions with different
//! algorithms that differ from fdlibm in the last 1-2 ULPs; that divergence is
//! visible in the color pipeline (hue/saturation digits), so the TUI color code
//! must call these ports instead of std.
//!
//! The code deliberately mirrors the C structure, including integer wraparound
//! arithmetic, so it can be audited line-by-line against the original. Original
//! fdlibm license (Sun Microsystems, freely granted) applies to the algorithm;
//! V8 modifications are Copyright 2016 the V8 project authors.

#![allow(
    // The constants below are verbatim fdlibm literals (20+ significant digits,
    // several of which approximate multiples of π); rewriting them to std's
    // shorter consts or truncating digits would break the bit-exact port.
    clippy::approx_constant,
    clippy::excessive_precision,
    // `x - x` is fdlibm's canonical way of producing NaN/±0 for inf/NaN inputs.
    clippy::eq_op
)]

// --- bit-level helpers (mirrors of the fdlibm C macros) ----------------------

#[inline]
fn high_word(x: f64) -> i32 {
    (x.to_bits() >> 32) as i32
}

#[inline]
fn low_word(x: f64) -> u32 {
    (x.to_bits() & 0xFFFF_FFFF) as u32
}

#[inline]
fn insert_words(hi: i32, lo: u32) -> f64 {
    f64::from_bits(((hi as u32 as u64) << 32) | lo as u64)
}

#[inline]
fn set_high_word(x: f64, v: i32) -> f64 {
    f64::from_bits((x.to_bits() & 0x0000_0000_FFFF_FFFF) | ((v as u32 as u64) << 32))
}

/// `scalbn(x, n)` = x * 2^n, computed exactly like the C libm version used by
/// fdlibm (repeated exact power-of-two multiplications, no rounding until the
/// result leaves the normal range).
fn scalbn(x: f64, n: i32) -> f64 {
    let mut y = x;
    let mut k = n;
    if k == 0 || !y.is_finite() || y == 0.0 {
        return y;
    }
    if k > 1023 {
        y *= f64::from_bits(0x7FE0_0000_0000_0000); // 2^1023
        k -= 1023;
        if k > 1023 {
            y *= f64::from_bits(0x7FE0_0000_0000_0000);
            k -= 1023;
        }
    } else if k < -1022 {
        // 2^-1023 via subnormal-safe steps: multiply by 2^-51 twice, then 2^-1022-ish
        y *= f64::from_bits(0x0010_0000_0000_0000); // 2^-1022
        k += 1022;
        if k < -1022 {
            y *= f64::from_bits(0x0010_0000_0000_0000);
            k += 1022;
        }
    }
    y * f64::from_bits((((1023 + k) as u64) & 0x7FF) << 52)
}

// --- 2/pi tables (396 hex digits of 2/pi, 24-bit chunks) ----------------------

const TWO_OVER_PI: [i32; 66] = [
    0xA2F983, 0x6E4E44, 0x1529FC, 0x2757D1, 0xF534DD, 0xC0DB62, 0x95993C, 0x439041, 0xFE5163,
    0xABDEBB, 0xC561B7, 0x246E3A, 0x424DD2, 0xE00649, 0x2EEA09, 0xD1921C, 0xFE1DEB, 0x1CB129,
    0xA73EE8, 0x8235F5, 0x2EBB44, 0x84E99C, 0x7026B4, 0x5F7E41, 0x3991D6, 0x398353, 0x39F49C,
    0x845F8B, 0xBDF928, 0x3B1FF8, 0x97FFDE, 0x05980F, 0xEF2F11, 0x8B5A0A, 0x6D1F6D, 0x367ECF,
    0x27CB09, 0xB74F46, 0x3F669E, 0x5FEA2D, 0x7527BA, 0xC7EBE5, 0xF17B3D, 0x0739F7, 0x8A5292,
    0xEA6BFB, 0x5FB11F, 0x8D5D08, 0x560330, 0x46FC7B, 0x6BABF0, 0xCFBC20, 0x9AF436, 0x1DA9E3,
    0x91615E, 0xE61B08, 0x659985, 0x5F14A0, 0x68408D, 0xFFD880, 0x4D7327, 0x310606, 0x1556CA,
    0x73A8C9, 0x60E27B, 0xC08C6B,
];

const NPIO2_HW: [i32; 32] = [
    0x3FF921FB, 0x400921FB, 0x4012D97C, 0x401921FB, 0x401F6A7A, 0x4022D97C, 0x4025FDBB,
    0x402921FB, 0x402C463A, 0x402F6A7A, 0x4031475C, 0x4032D97C, 0x40346B9C, 0x4035FDBB,
    0x40378FDB, 0x403921FB, 0x403AB41B, 0x403C463A, 0x403DD85A, 0x403F6A7A, 0x40407E4C,
    0x4041475C, 0x4042106C, 0x4042D97C, 0x4043A28C, 0x40446B9C, 0x404534AC, 0x4045FDBB,
    0x4046C6CB, 0x40478FDB, 0x404858EB, 0x404921FB,
];

/// `__ieee754_rem_pio2(x, y)`: remainder of x mod pi/2 in `y[0] + y[1]`.
/// Returns the last three binary digits of `N` where `x = N*pi/2 + y`.
fn ieee754_rem_pio2(x: f64, y: &mut [f64; 2]) -> i32 {
    const ZERO: f64 = 0.0;
    const HALF: f64 = 5.00000000000000000000e-01;
    const TWO24: f64 = 1.67772160000000000000e+07;
    const INVPIO2: f64 = 6.36619772367581382433e-01;
    const PIO2_1: f64 = 1.57079632673412561417e+00;
    const PIO2_1T: f64 = 6.07710050650619224932e-11;
    const PIO2_2: f64 = 6.07710050630396597660e-11;
    const PIO2_2T: f64 = 2.02226624879595063154e-21;
    const PIO2_3: f64 = 2.02226624871116645580e-21;
    const PIO2_3T: f64 = 8.47842766036889956997e-32;

    let mut z: f64;
    let mut w: f64;
    let mut t: f64;
    let mut r: f64;
    let fn_: f64;
    let mut tx = [0.0f64; 3];
    let e0: i32;
    let mut i: i32;
    let j: i32;
    let mut nx: i32;
    let n: i32;
    let ix: i32;
    let hx = high_word(x);
    ix = hx & 0x7FFF_FFFF;

    if ix <= 0x3FE9_21FB {
        // |x| ~<= pi/4, no need for reduction
        y[0] = x;
        y[1] = 0.0;
        return 0;
    }
    if ix < 0x4002_D97C {
        // |x| < 3pi/4, special case with n = +-1
        if hx > 0 {
            z = x - PIO2_1;
            if ix != 0x3FF9_21FB {
                // 33+53 bit pi is good enough
                y[0] = z - PIO2_1T;
                y[1] = (z - y[0]) - PIO2_1T;
            } else {
                // near pi/2, use 33+33+53 bit pi
                z -= PIO2_2;
                y[0] = z - PIO2_2T;
                y[1] = (z - y[0]) - PIO2_2T;
            }
            return 1;
        } else {
            // negative x
            z = x + PIO2_1;
            if ix != 0x3FF9_21FB {
                y[0] = z + PIO2_1T;
                y[1] = (z - y[0]) + PIO2_1T;
            } else {
                z += PIO2_2;
                y[0] = z + PIO2_2T;
                y[1] = (z - y[0]) + PIO2_2T;
            }
            return -1;
        }
    }
    if ix <= 0x4139_21FB {
        // |x| ~<= 2^19*(pi/2), medium size
        t = x.abs();
        n = (t * INVPIO2 + HALF) as i32;
        fn_ = n as f64;
        r = t - fn_ * PIO2_1;
        w = fn_ * PIO2_1T; // 1st round good to 85 bit
        if n < 32 && ix != NPIO2_HW[(n - 1) as usize] {
            y[0] = r - w; // quick check no cancellation
        } else {
            j = ix >> 20;
            y[0] = r - w;
            let mut high = high_word(y[0]);
            i = j - ((high >> 20) & 0x7FF);
            if i > 16 {
                // 2nd iteration needed, good to 118
                t = r;
                w = fn_ * PIO2_2;
                r = t - w;
                w = fn_ * PIO2_2T - ((t - r) - w);
                y[0] = r - w;
                high = high_word(y[0]);
                i = j - ((high >> 20) & 0x7FF);
                if i > 49 {
                    // 3rd iteration needed, 151 bits acc
                    t = r; // will cover all possible cases
                    w = fn_ * PIO2_3;
                    r = t - w;
                    w = fn_ * PIO2_3T - ((t - r) - w);
                    y[0] = r - w;
                }
            }
        }
        y[1] = (r - y[0]) - w;
        if hx < 0 {
            y[0] = -y[0];
            y[1] = -y[1];
            return -n;
        } else {
            return n;
        }
    }
    // all other (large) arguments
    if ix >= 0x7FF0_0000 {
        // x is inf or NaN
        y[0] = x - x;
        y[1] = x - x;
        return 0;
    }
    // set z = scalbn(|x|, ilogb(x)-23)
    let low = low_word(x);
    z = f64::from_bits(low as u64); // SET_LOW_WORD(z=0, low)
    e0 = (ix >> 20) - 1046; // e0 = ilogb(z)-23
    z = set_high_word(z, (ix as i32).wrapping_sub(((e0 as u32) << 20) as i32));
    for i in 0..2 {
        tx[i] = (z as i32) as f64;
        z = (z - tx[i]) * TWO24;
    }
    tx[2] = z;
    nx = 3;
    while tx[(nx - 1) as usize] == ZERO {
        nx -= 1; // skip zero term
    }
    n = kernel_rem_pio2(&mut tx, y, e0, nx, 2, &TWO_OVER_PI);
    if hx < 0 {
        y[0] = -y[0];
        y[1] = -y[1];
        return -n;
    }
    n
}

/// `__kernel_rem_pio2(x, y, e0, nx, prec, ipio2)`.
fn kernel_rem_pio2(x: &mut [f64; 3], y: &mut [f64; 2], e0: i32, nx: i32, prec: i32, ipio2: &[i32; 66]) -> i32 {
    const INIT_JK: [i32; 4] = [2, 3, 4, 6]; // initial value for jk

    const PIO2: [f64; 8] = [
        1.57079625129699707031e+00,
        7.54978941586159635335e-08,
        5.39030252995776476554e-15,
        3.28200341580791294123e-22,
        1.27065575308067607349e-29,
        1.22933308981111328932e-36,
        2.73370053816464559624e-44,
        2.16741683877804819444e-51,
    ];

    const ZERO: f64 = 0.0;
    const ONE: f64 = 1.0;
    const TWO24: f64 = 1.67772160000000000000e+07;
    const TWON24: f64 = 5.96046447753906250000e-08;

    let mut f = [0.0f64; 20];
    let mut fq = [0.0f64; 20];
    let mut q = [0.0f64; 20];
    let mut iq = [0i32; 20];

    // initialize jk
    let jk = INIT_JK[prec as usize];
    let jp = jk;

    // determine jx, jv, q0, note that 3 > q0
    let jx = nx - 1;
    let mut jv = (e0 - 3) / 24;
    if jv < 0 {
        jv = 0;
    }
    let mut q0 = e0 - 24 * (jv + 1);

    // set up f[0] to f[jx+jk] where f[jx+jk] = ipio2[jv+jk]
    let mut j = jv - jx;
    let m = jx + jk;
    let mut i = 0;
    while i <= m {
        f[i as usize] = if j < 0 {
            ZERO
        } else {
            ipio2[j as usize] as f64
        };
        i += 1;
        j += 1;
    }

    // compute q[0], q[1], ... q[jk]
    for i in 0..=jk {
        let mut fw = 0.0;
        for j in 0..=jx {
            fw += x[j as usize] * f[(jx + i - j) as usize];
        }
        q[i as usize] = fw;
    }

    let mut jz = jk;
    'recompute: loop {
        // distill q[] into iq[] reversingly
        let mut z;
        {
            let mut i = 0;
            let mut j = jz;
            z = q[jz as usize];
            while j > 0 {
                let fw = (TWON24 * z) as i32 as f64;
                iq[i as usize] = (z - TWO24 * fw) as i32;
                z = q[(j - 1) as usize] + fw;
                i += 1;
                j -= 1;
            }
        }

        // compute n
        z = scalbn(z, q0); // actual value of z
        z -= 8.0 * (z * 0.125).floor(); // trim off integer >= 8
        let mut n = z as i32;
        z -= n as f64;
        let mut ih = 0;
        if q0 > 0 {
            // need iq[jz-1] to determine n
            i = iq[(jz - 1) as usize] >> (24 - q0);
            n += i;
            iq[(jz - 1) as usize] -= i << (24 - q0);
            ih = iq[(jz - 1) as usize] >> (23 - q0);
        } else if q0 == 0 {
            ih = iq[(jz - 1) as usize] >> 23;
        } else if z >= 0.5 {
            ih = 2;
        }

        if ih > 0 {
            // q > 0.5
            n += 1;
            let mut carry = 0;
            for i in 0..jz {
                // compute 1-q
                let j = iq[i as usize];
                if carry == 0 {
                    if j != 0 {
                        carry = 1;
                        iq[i as usize] = 0x100_0000 - j;
                    }
                } else {
                    iq[i as usize] = 0xFF_FFFF - j;
                }
            }
            if q0 > 0 {
                // rare case: chance is 1 in 12
                match q0 {
                    1 => iq[(jz - 1) as usize] &= 0x7F_FFFF,
                    2 => iq[(jz - 1) as usize] &= 0x3F_FFFF,
                    _ => {}
                }
            }
            if ih == 2 {
                z = ONE - z;
                if carry != 0 {
                    z -= scalbn(ONE, q0);
                }
            }
        }

        // check if recomputation is needed
        if z == ZERO {
            let mut j = 0;
            let mut i = jz - 1;
            while i >= jk {
                j |= iq[i as usize];
                i -= 1;
            }
            if j == 0 {
                // need recomputation
                let mut k = 1;
                while jk >= k && iq[(jk - k) as usize] == 0 {
                    k += 1; // k = no. of terms needed
                }

                for i in jz + 1..=jz + k {
                    // add q[jz+1] to q[jz+k]
                    f[(jx + i) as usize] = ipio2[(jv + i) as usize] as f64;
                    let mut fw = 0.0;
                    for j in 0..=jx {
                        fw += x[j as usize] * f[(jx + i - j) as usize];
                    }
                    q[i as usize] = fw;
                }
                jz += k;
                continue 'recompute;
            }
        }

        // chop off zero terms
        if z == 0.0 {
            jz -= 1;
            q0 -= 24;
            while iq[jz as usize] == 0 {
                jz -= 1;
                q0 -= 24;
            }
        } else {
            // break z into 24-bit if necessary
            z = scalbn(z, -q0);
            if z >= TWO24 {
                let fw = (TWON24 * z) as i32 as f64;
                iq[jz as usize] = (z - TWO24 * fw) as i32;
                jz += 1;
                q0 += 24;
                iq[jz as usize] = fw as i32;
            } else {
                iq[jz as usize] = z as i32;
            }
        }

        // convert integer "bit" chunk to floating-point value
        let mut fw = scalbn(ONE, q0);
        let mut i = jz;
        while i >= 0 {
            q[i as usize] = fw * iq[i as usize] as f64;
            fw *= TWON24;
            i -= 1;
        }

        // compute PIo2[0,...,jp]*q[jz,...,0]
        let mut i = jz;
        while i >= 0 {
            let mut fw = 0.0;
            let mut k = 0;
            while k <= jp && k <= jz - i {
                fw += PIO2[k as usize] * q[(i + k) as usize];
                k += 1;
            }
            fq[(jz - i) as usize] = fw;
            i -= 1;
        }

        // compress fq[] into y[]
        match prec {
            0 => {
                let mut fw = 0.0;
                let mut i = jz;
                while i >= 0 {
                    fw += fq[i as usize];
                    i -= 1;
                }
                y[0] = if ih == 0 { fw } else { -fw };
            }
            1 | 2 => {
                let mut fw = 0.0;
                let mut i = jz;
                while i >= 0 {
                    fw += fq[i as usize];
                    i -= 1;
                }
                y[0] = if ih == 0 { fw } else { -fw };
                fw = fq[0] - fw;
                for i in 1..=jz {
                    fw += fq[i as usize];
                }
                y[1] = if ih == 0 { fw } else { -fw };
            }
            _ => {
                // 3: long double precision (unused by the double sin/cos path)
                unimplemented!("prec=3 not needed for Math.sin/Math.cos");
            }
        }
        return n & 7;
    }
}

/// `__kernel_cos(x, y)`: kernel cos function on [-pi/4, pi/4]; y is the tail
/// of x.
fn kernel_cos(x: f64, y: f64) -> f64 {
    const ONE: f64 = 1.00000000000000000000e+00;
    const C1: f64 = 4.16666666666666019037e-02;
    const C2: f64 = -1.38888888888741095749e-03;
    const C3: f64 = 2.48015872894767294178e-05;
    const C4: f64 = -2.75573143513906633035e-07;
    const C5: f64 = 2.08757232129817482790e-09;
    const C6: f64 = -1.13596475577881948265e-11;

    let mut ix = high_word(x);
    ix &= 0x7FFF_FFFF; // ix = |x|'s high word
    if ix < 0x3E40_0000 {
        // if x < 2**27
        if x as i32 == 0 {
            return ONE; // generate inexact
        }
    }
    let z = x * x;
    let r = z * (C1 + z * (C2 + z * (C3 + z * (C4 + z * (C5 + z * C6)))));
    if ix < 0x3FD3_3333 {
        // if |x| < 0.3
        ONE - (0.5 * z - (z * r - x * y))
    } else {
        let qx;
        if ix > 0x3FE9_0000 {
            // x > 0.78125
            qx = 0.28125;
        } else {
            qx = insert_words(ix - 0x0020_0000, 0); // x/4
        }
        let iz = 0.5 * z - qx;
        let a = ONE - qx;
        a - (iz - (z * r - x * y))
    }
}

/// `__kernel_sin(x, y, iy)`: kernel sin function on [-pi/4, pi/4]; y is the
/// tail of x; iy indicates whether y is 0.
fn kernel_sin(x: f64, y: f64, iy: i32) -> f64 {
    const HALF: f64 = 5.00000000000000000000e-01;
    const S1: f64 = -1.66666666666666324348e-01;
    const S2: f64 = 8.33333333332248946124e-03;
    const S3: f64 = -1.98412698298579493134e-04;
    const S4: f64 = 2.75573137070700676789e-06;
    const S5: f64 = -2.50507602534068634195e-08;
    const S6: f64 = 1.58969099521155010221e-10;

    let mut ix = high_word(x);
    ix &= 0x7FFF_FFFF; // high word of x
    if ix < 0x3E40_0000 {
        // |x| < 2**-27
        if x as i32 == 0 {
            return x; // generate inexact
        }
    }
    let z = x * x;
    let v = z * x;
    let r = S2 + z * (S3 + z * (S4 + z * (S5 + z * S6)));
    if iy == 0 {
        x + v * (S1 + z * r)
    } else {
        x - ((z * (HALF * y - v * r) - y) - v * S1)
    }
}

/// `Math.atan` — fdlibm `s_atan.c` (via V8's ieee754.cc).
pub fn atan(x: f64) -> f64 {
    const ATANHI: [f64; 4] = [
        4.63647609000806093515e-01, // atan(0.5)hi
        7.85398163397448278999e-01, // atan(1.0)hi
        9.82793723247329054082e-01, // atan(1.5)hi
        1.57079632679489655800e+00, // atan(inf)hi
    ];

    const ATANLO: [f64; 4] = [
        2.26987774529616870924e-17, // atan(0.5)lo
        3.06161699786838301793e-17, // atan(1.0)lo
        1.39033110312309984516e-17, // atan(1.5)lo
        6.12323399573676603587e-17, // atan(inf)lo
    ];

    const AT: [f64; 11] = [
        3.33333333333329318027e-01,
        -1.99999999998764832476e-01,
        1.42857142725034663711e-01,
        -1.11111104054623557880e-01,
        9.09088713343650656196e-02,
        -7.69187620504482999495e-02,
        6.66107313738753120669e-02,
        -5.83357013379057348645e-02,
        4.97687799461593236017e-02,
        -3.65315727442169155270e-02,
        1.62858201153657823623e-02,
    ];

    const ONE: f64 = 1.0;
    const HUGE: f64 = 1.0e300;

    let mut x = x;
    let hx = high_word(x);
    let ix = hx & 0x7FFF_FFFF;
    let id: i32;
    if ix >= 0x4410_0000 {
        // if |x| >= 2^66
        let low = low_word(x);
        if ix > 0x7FF0_0000 || (ix == 0x7FF0_0000 && low != 0) {
            return x + x; // NaN
        }
        if hx > 0 {
            return ATANHI[3] + ATANLO[3];
        } else {
            return -ATANHI[3] - ATANLO[3];
        }
    }
    if ix < 0x3FDC_0000 {
        // |x| < 0.4375
        if ix < 0x3E40_0000 {
            // |x| < 2^-27
            if HUGE + x > ONE {
                return x; // raise inexact
            }
        }
        id = -1;
    } else {
        x = x.abs();
        if ix < 0x3FF3_0000 {
            // |x| < 1.1875
            if ix < 0x3FE6_0000 {
                // 7/16 <= |x| < 11/16
                id = 0;
                x = (2.0 * x - ONE) / (2.0 + x);
            } else {
                // 11/16 <= |x| < 19/16
                id = 1;
                x = (x - ONE) / (x + ONE);
            }
        } else if ix < 0x4003_8000 {
            // |x| < 2.4375
            id = 2;
            x = (x - 1.5) / (ONE + 1.5 * x);
        } else {
            // 2.4375 <= |x| < 2^66
            id = 3;
            x = -1.0 / x;
        }
    }
    // end of argument reduction
    let z = x * x;
    let w = z * z;
    // break sum from i=0 to 10 aT[i]z**(i+1) into odd and even poly
    let s1 = z * (AT[0] + w * (AT[2] + w * (AT[4] + w * (AT[6] + w * (AT[8] + w * AT[10])))));
    let s2 = w * (AT[1] + w * (AT[3] + w * (AT[5] + w * (AT[7] + w * AT[9]))));
    if id < 0 {
        x - x * (s1 + s2)
    } else {
        let z = ATANHI[id as usize] - ((x * (s1 + s2) - ATANLO[id as usize]) - x);
        if hx < 0 {
            -z
        } else {
            z
        }
    }
}

/// `Math.atan2` — fdlibm `e_atan2.c` (via V8's ieee754.cc).
pub fn atan2(y: f64, x: f64) -> f64 {
    const TINY: f64 = 1.0e-300;
    const ZERO: f64 = 0.0;
    const PI_O_4: f64 = 7.8539816339744827900E-01;
    const PI_O_2: f64 = 1.5707963267948965580E+00;
    const PI: f64 = 3.1415926535897931160E+00;
    const PI_LO: f64 = 1.2246467991473531772E-16;

    let hx = high_word(x);
    let lx = low_word(x);
    let ix = (hx & 0x7FFF_FFFF) as u32;
    let hy = high_word(y);
    let ly = low_word(y);
    let iy = (hy & 0x7FFF_FFFF) as u32;
    if (ix | ((lx | lx.wrapping_neg()) >> 31)) > 0x7FF0_0000
        || (iy | ((ly | ly.wrapping_neg()) >> 31)) > 0x7FF0_0000
    {
        return x + y; // x or y is NaN
    }
    if ((hx.wrapping_sub(0x3FF0_0000) as u32) | lx) == 0 {
        return atan(y); // x = 1.0
    }
    let mut m = ((hy >> 31) & 1) | ((hx >> 30) & 2); // 2*sign(x)+sign(y)

    // when y = 0
    if (iy | ly) == 0 {
        match m {
            0 | 1 => return y,          // atan(+-0, +anything) = +-0
            2 => return PI + TINY,      // atan(+0, -anything) = pi
            _ => return -PI - TINY,     // atan(-0, -anything) = -pi
        }
    }
    // when x = 0
    if (ix | lx) == 0 {
        return if hy < 0 { -PI_O_2 - TINY } else { PI_O_2 + TINY };
    }

    // when x is INF
    if ix == 0x7FF0_0000 {
        if iy == 0x7FF0_0000 {
            match m {
                0 => return PI_O_4 + TINY,       // atan(+INF, +INF)
                1 => return -PI_O_4 - TINY,      // atan(-INF, +INF)
                2 => return 3.0 * PI_O_4 + TINY, // atan(+INF, -INF)
                _ => return -3.0 * PI_O_4 - TINY, // atan(-INF, -INF)
            }
        } else {
            match m {
                0 => return ZERO,          // atan(+..., +INF)
                1 => return -ZERO,         // atan(-..., +INF)
                2 => return PI + TINY,     // atan(+..., -INF)
                _ => return -PI - TINY,    // atan(-..., -INF)
            }
        }
    }
    // when y is INF
    if iy == 0x7FF0_0000 {
        return if hy < 0 { -PI_O_2 - TINY } else { PI_O_2 + TINY };
    }

    // compute y/x
    let k = (iy as i32 - ix as i32) >> 20;
    let z;
    if k > 60 {
        // |y/x| > 2**60
        z = PI_O_2 + 0.5 * PI_LO;
        m &= 1;
    } else if hx < 0 && k < -60 {
        z = 0.0; // 0 > |y|/x > -2**-60
    } else {
        z = atan((y / x).abs()); // safe to do y/x
    }
    match m {
        0 => z,                 // atan(+, +)
        1 => -z,                // atan(-, +)
        2 => PI - (z - PI_LO),  // atan(+, -)
        _ => (z - PI_LO) - PI,  // atan(-, -)
    }
}

/// `Math.sin` — fdlibm `s_sin.c` (via V8's ieee754.cc).
pub fn sin(x: f64) -> f64 {
    let mut y = [0.0f64; 2];
    let z = 0.0;
    let mut ix = high_word(x);

    // |x| ~< pi/4
    ix &= 0x7FFF_FFFF;
    if ix <= 0x3FE9_21FB {
        kernel_sin(x, z, 0)
    } else if ix >= 0x7FF0_0000 {
        // sin(Inf or NaN) is NaN
        x - x
    } else {
        // argument reduction needed
        let n = ieee754_rem_pio2(x, &mut y);
        match n & 3 {
            0 => kernel_sin(y[0], y[1], 1),
            1 => kernel_cos(y[0], y[1]),
            2 => -kernel_sin(y[0], y[1], 1),
            _ => -kernel_cos(y[0], y[1]),
        }
    }
}

/// `Math.cos` — fdlibm `s_cos.c` (via V8's ieee754.cc).
pub fn cos(x: f64) -> f64 {
    let mut y = [0.0f64; 2];
    let z = 0.0;
    let mut ix = high_word(x);

    // |x| ~< pi/4
    ix &= 0x7FFF_FFFF;
    if ix <= 0x3FE9_21FB {
        kernel_cos(x, z)
    } else if ix >= 0x7FF0_0000 {
        // cos(Inf or NaN) is NaN
        x - x
    } else {
        // argument reduction needed
        let n = ieee754_rem_pio2(x, &mut y);
        match n & 3 {
            0 => kernel_cos(y[0], y[1]),
            1 => -kernel_sin(y[0], y[1], 1),
            2 => -kernel_cos(y[0], y[1]),
            _ => kernel_sin(y[0], y[1], 1),
        }
    }
}

/// `Math.cbrt` — fdlibm `s_cbrt.c` (via V8's ieee754.cc).
pub fn cbrt(x: f64) -> f64 {
    const B1: u32 = 715_094_163; // (1023-1023/3-0.03306235651)*2**20
    const B2: u32 = 696_219_795; // (1023-1023/3-54/3-0.03306235651)*2**20

    // |1/cbrt(x) - p(x)| < 2**-23.5 (~[-7.93e-8, 7.929e-8])
    const P0: f64 = 1.87595182427177009643;
    const P1: f64 = -1.88497979543377169875;
    const P2: f64 = 1.621429720105354466140;
    const P3: f64 = -0.758397934778766047437;
    const P4: f64 = 0.145996192886612446982;

    let mut t: f64;
    let hx = high_word(x);
    let low = low_word(x);
    let sign = (hx as u32) & 0x8000_0000; // sign = sign(x)
    let hx = (hx as u32 ^ sign) as i32;
    if hx as u32 >= 0x7FF0_0000 {
        return x + x; // cbrt(NaN, INF) is itself
    }

    // Rough cbrt to 5 bits (see original comments in s_cbrt.c).
    if hx < 0x0010_0000 {
        // zero or subnormal?
        if (hx as u32 | low) == 0 {
            return x; // cbrt(0) is itself
        }
        t = f64::from_bits(0x4350_0000_0000_0000); // set t = 2**54
        t *= x;
        let high = high_word(t);
        t = insert_words((sign | ((high as u32 & 0x7FFF_FFFF) / 3 + B2)) as i32, 0);
    } else {
        t = insert_words((sign | (hx as u32 / 3 + B1)) as i32, 0);
    }

    // New cbrt to 23 bits: cbrt(x) = t*cbrt(x/t**3) ~= t*P(t**3/x)
    let mut r = (t * t) * (t / x);
    t = t * ((P0 + r * (P1 + r * P2)) + ((r * r) * r) * (P3 + r * P4));

    // Round t away from zero to 23 bits
    let mut bits = t.to_bits();
    bits = (bits + 0x8000_0000) & 0xFFFF_FFFF_C000_0000;
    t = f64::from_bits(bits);

    // one step Newton iteration to 53 bits with error < 0.667 ulps
    let s = t * t; // t*t is exact
    r = x / s; // error <= 0.5 ulps; |r| < |t|
    let w = t + t; // t+t is exact
    r = (r - t) / (w + r); // r-t is exact; w+r ~= 3*t
    t + t * r // error <= 0.5 + 0.5/3 + epsilon
}

/// `Math.hypot` — V8's `FastMathHypot` Torque builtin (2-argument case, the
/// only shape the TUI color code uses). Scales by the max magnitude so the
/// squares are computed on values in [0, 1], then scales back.
pub fn hypot(x: f64, y: f64) -> f64 {
    let a = x.abs();
    let b = y.abs();
    if a == f64::INFINITY || b == f64::INFINITY {
        return f64::INFINITY;
    }
    let max = if a > b { a } else { b };
    if max.is_nan() {
        return f64::NAN;
    }
    if max == 0.0 {
        return 0.0;
    }
    ((a / max) * (a / max) + (b / max) * (b / max)).sqrt() * max
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compare against V8-produced reference values (computed with Node).
    #[test]
    fn matches_node_reference_values() {
        // Generated with: node -e 'console.log(Math.atan2(...).toString(2))'
        // Spot values across the interesting ranges.
        assert_eq!(atan2(1.0, 2.0).to_bits(), 0x3FDDAC670561BB4F);
        assert_eq!(atan2(-1.0, 2.0).to_bits(), 0xBFDDAC670561BB4F);
        assert_eq!(atan2(1.0, -2.0).to_bits(), 0x40056C6E7397F5AE);
        assert_eq!(atan2(0.4809345314400326, 0.02897262682471269).to_bits(), 0x3FF82B86E5554410);
        assert_eq!(atan2(0.6141707790052229, -0.5202806468753018).to_bits(), 0x4002305FBBD38200);
        assert_eq!(sin(1.2957409547783927).to_bits(), 0x3FEECC10645846D9);
        assert_eq!(sin(2.3861895747392154e-307).to_bits(), 0x004572BC206C73FD);
        assert_eq!(cos(0.9648986460818719).to_bits(), 0x3FE239587711BD09);
        assert_eq!(cos(3.9980273879569133).to_bits(), 0xBFE4F6DE26341ED0);
        assert_eq!(cbrt(-0.7372079239367317).to_bits(), 0xBFECE85E070CABAA);
        assert_eq!(cbrt(1.5563728631690855e-298).to_bits(), 0x2B52D311718F6E19);
        assert_eq!(hypot(0.13336238251905452, 0.42350188175877623).to_bits(), 0x3FDC6A8EE36FC3E5);
        assert_eq!(hypot(-0.12249456161475236, -0.38605417966635047).to_bits(), 0x3FD9EBE103FBB111);
    }

    #[test]
    fn special_values() {
        // sin/cos of inf are NaN
        assert!(sin(f64::INFINITY).is_nan());
        assert!(cos(f64::NEG_INFINITY).is_nan());
        // cbrt/hypot of zero
        assert_eq!(cbrt(0.0).to_bits(), 0.0f64.to_bits());
        assert_eq!(cbrt(-0.0).to_bits(), (-0.0f64).to_bits());
        assert_eq!(hypot(0.0, -0.0).to_bits(), 0.0f64.to_bits());
        // hypot(inf, nan) = inf per the fast path
        assert_eq!(hypot(f64::NAN, f64::INFINITY), f64::INFINITY);
        assert!(hypot(f64::NAN, 5.0).is_nan());
        // atan2 special cases
        assert_eq!(atan2(0.0, -1.0), std::f64::consts::PI + 1.0e-300);
        assert_eq!(atan2(1.0, f64::INFINITY), 0.0);
        assert_eq!(atan2(f64::INFINITY, f64::INFINITY), std::f64::consts::FRAC_PI_4 + 1.0e-300);
    }
}
