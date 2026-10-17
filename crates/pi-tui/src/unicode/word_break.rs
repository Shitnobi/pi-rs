//! ICU 78.3 word-break segmentation (Node 24.22.0's `Intl.Segmenter`, word
//! granularity) — 1:1 port of the validated prototype
//! `scripts/lib/word-segmenter.mjs` (20099/0 + 3×100k fuzz bit-exact vs the
//! live `Intl.Segmenter`).
//!
//! Executes the actual ICU machinery:
//!   - word.brk RBBI forward state machine (`handle_next`)
//!   - UCPTrie category lookup (range-compressed table)
//!   - DictionaryBreakEngine dispatch by script (brkeng.cpp) with ICU's
//!     process-global factory cache + per-call instance stack semantics
//!   - Thai/Lao/Khmer/Burmese PossibleWord engines (dictbe.cpp), including
//!     the Thai code-point vs Lao/Khmer/Burmese code-unit MIN_WORD_SPAN split
//!   - CjkBreakEngine Viterbi DP with NFKC normalization + katakana costs
//!   - BytesTrie / UCharsTrie dictionary walkers (exact node arithmetic)
//!   - DictionaryCache population + forward iteration (rbbi_cache.cpp)
//!
//! All positions are UTF-16 code-unit indices (UText native indices), exactly
//! like the ICU original; the public API converts to/from `&str`.

use std::sync::{Mutex, OnceLock};

use super::in_ranges;
use super::word_break_data::*;

// ---------------------------------------------------------------------------
// Trie result codes (ustringtrie.h)
// ---------------------------------------------------------------------------
const NO_MATCH: i32 = 0;
const NO_VALUE: i32 = 1;
const FINAL_VALUE: i32 = 2;
const INTERMEDIATE_VALUE: i32 = 3;

// ---------------------------------------------------------------------------
// UText over UTF-16 code units (native index = code-unit index)
// ---------------------------------------------------------------------------
fn cp_at(s: &[u16], i: usize) -> i32 {
    if i >= s.len() {
        return -1;
    }
    let c = s[i] as u32;
    if (0xd800..=0xdbff).contains(&c) && i + 1 < s.len() {
        let c2 = s[i + 1] as u32;
        if (0xdc00..=0xdfff).contains(&c2) {
            return ((c - 0xd800) * 0x400 + (c2 - 0xdc00) + 0x10000) as i32;
        }
    }
    c as i32
}
fn cp_end_at(s: &[u16], i: usize) -> i32 {
    if i == 0 || i > s.len() {
        return -1;
    }
    let c1 = s[i - 1] as u32;
    if (0xdc00..=0xdfff).contains(&c1) && i >= 2 {
        let c0 = s[i - 2] as u32;
        if (0xd800..=0xdbff).contains(&c0) {
            return ((c0 - 0xd800) * 0x400 + (c1 - 0xdc00) + 0x10000) as i32;
        }
    }
    c1 as i32
}
fn cp_size(c: i32) -> usize {
    if c > 0xffff {
        2
    } else {
        1
    }
}
fn encode_utf16(cps: &[u32], out: &mut Vec<u16>) {
    for &c in cps {
        if c > 0xffff {
            let c = c - 0x10000;
            out.push((0xd800 + (c >> 10)) as u16);
            out.push((0xdc00 + (c & 0x3ff)) as u16);
        } else {
            out.push(c as u16);
        }
    }
}

struct UText<'a> {
    s: &'a [u16],
    i: usize,
}

impl<'a> UText<'a> {
    fn new(s: &'a [u16]) -> Self {
        UText { s, i: 0 }
    }
    fn set_index(&mut self, i: usize) {
        self.i = i;
    }
    fn index(&self) -> usize {
        self.i
    }
    fn current32(&self) -> i32 {
        cp_at(self.s, self.i)
    }
    fn next32(&mut self) -> i32 {
        let c = cp_at(self.s, self.i);
        if c >= 0 {
            self.i += cp_size(c);
        }
        c
    }
    fn previous32(&mut self) -> i32 {
        let c = cp_end_at(self.s, self.i);
        if c >= 0 {
            self.i -= cp_size(c);
        }
        c
    }
    fn move_index32(&mut self, n: i32) {
        let mut n = n;
        while n > 0 && self.i < self.s.len() {
            self.i += cp_size(cp_at(self.s, self.i));
            n -= 1;
        }
        while n < 0 && self.i > 0 {
            self.i -= cp_size(cp_end_at(self.s, self.i));
            n += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Binary-searched generated tables
// ---------------------------------------------------------------------------

/// UCPTrie category for a code point (word.brk trie, range-compressed).
fn category_of(cp: u32) -> u8 {
    let rs = CATEGORY_RANGES;
    let mut lo = 0usize;
    let mut hi = rs.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        let (start, end, v) = rs[mid];
        if cp < start {
            hi = mid;
        } else if cp > end {
            lo = mid + 1;
        } else {
            return v;
        }
    }
    0
}

/// Scripts.txt script id for a code point (SCRIPT_UNKNOWN for gaps).
fn script_of(cp: u32) -> u8 {
    let rs = SCRIPT_RANGES;
    let mut lo = 0usize;
    let mut hi = rs.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        let (start, end, v) = rs[mid];
        if cp < start {
            hi = mid;
        } else if cp > end {
            lo = mid + 1;
        } else {
            return v;
        }
    }
    SCRIPT_UNKNOWN
}

/// All ranges of a script (UnicodeSet::applyIntPropertyValue(UCHAR_SCRIPT, s)).
fn script_ranges_of(script: u8) -> Vec<(u32, u32)> {
    SCRIPT_RANGES
        .iter()
        .filter(|&&(_, _, v)| v == script)
        .map(|&(a, b, _)| (a, b))
        .collect()
}

// ---------------------------------------------------------------------------
// NFKC (table-driven; mirrors buildNfkc() in the prototype)
// ---------------------------------------------------------------------------
fn ccc(c: u32) -> u8 {
    let rs = CCC_RANGES;
    let mut lo = 0usize;
    let mut hi = rs.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        let (start, end, v) = rs[mid];
        if c < start {
            hi = mid;
        } else if c > end {
            lo = mid + 1;
        } else {
            return v;
        }
    }
    0
}

/// Full recursive compat decomposition (Hangul is algorithmic).
fn decompose_cp(c: u32, out: &mut Vec<u32>) {
    if (0xac00..=0xd7a3).contains(&c) {
        let si = c - 0xac00;
        out.push(0x1100 + si / 588);
        out.push(0x1161 + (si % 588) / 28);
        let t = 0x11a7 + si % 28;
        if t != 0x11a7 {
            out.push(t);
        }
        return;
    }
    let keys = NFKC_DECOMP_KEYS;
    let mut lo = 0usize;
    let mut hi = keys.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        if c < keys[mid] {
            hi = mid;
        } else if c > keys[mid] {
            lo = mid + 1;
        } else {
            let a = NFKC_DECOMP_OFFS[mid] as usize;
            let b = NFKC_DECOMP_OFFS[mid + 1] as usize;
            out.extend_from_slice(&NFKC_DECOMP_FLAT[a..b]);
            return;
        }
    }
    out.push(c);
}

/// Canonical composition pair (Hangul algorithmic + NFKC_PAIRS). -1 if none.
fn compose_pair(x: u32, y: u32) -> i64 {
    if (0x1100..=0x1112).contains(&x) && (0x1161..=0x1175).contains(&y) {
        return (0xac00 + ((x - 0x1100) * 21 + (y - 0x1161)) * 28) as i64;
    }
    if (0xac00..=0xd7a3).contains(&x) && (x - 0xac00) % 28 == 0 && (0x11a8..=0x11c2).contains(&y) {
        return (x + (y - 0x11a7)) as i64;
    }
    let ps = NFKC_PAIRS;
    let mut lo = 0usize;
    let mut hi = ps.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        let (px, py, pv) = ps[mid];
        if x < px || (x == px && y < py) {
            hi = mid;
        } else if x > px || (x == px && y > py) {
            lo = mid + 1;
        } else {
            return pv as i64;
        }
    }
    -1
}

fn canonical_order(seq: &mut [u32]) {
    // stable insertion sort by ccc
    for i in 1..seq.len() {
        let ci = ccc(seq[i]);
        if ci == 0 {
            continue;
        }
        let mut j = i;
        while j > 0 && ccc(seq[j - 1]) > ci {
            seq.swap(j - 1, j);
            j -= 1;
        }
    }
}

/// NFKC over UTF-16 code units → UTF-16 code units.
fn nfkc(s: &[u16]) -> Vec<u16> {
    let mut dec: Vec<u32> = Vec::new();
    let mut i = 0usize;
    while i < s.len() {
        let c = cp_at(s, i);
        decompose_cp(c as u32, &mut dec);
        i += cp_size(c);
    }
    canonical_order(&mut dec);
    let mut out: Vec<u32> = Vec::with_capacity(dec.len());
    let mut starter_pos: i64 = -1;
    let mut starter_ch = 0u32;
    let mut last_class = 0u8;
    let mut have_starter = false;
    for &ch in &dec {
        let cl = ccc(ch);
        let mut comp: i64 = -1;
        if have_starter && (last_class < cl || last_class == 0) {
            comp = compose_pair(starter_ch, ch);
        }
        if comp >= 0 {
            starter_ch = comp as u32;
            out[starter_pos as usize] = starter_ch;
            continue;
        }
        out.push(ch);
        if cl == 0 {
            starter_pos = out.len() as i64 - 1;
            starter_ch = ch;
            have_starter = true;
        }
        last_class = cl;
    }
    let mut r: Vec<u16> = Vec::with_capacity(out.len());
    encode_utf16(&out, &mut r);
    r
}

/// ICU nfkcNorm2->hasBoundaryBefore(c) — precomputed (includes the
/// backward-combining-decomposition-start rule for FF9E/FF9F etc.).
fn has_boundary_before(c: u32) -> bool {
    !in_ranges(NFKC_NO_BOUNDARY_BEFORE, c)
}

// ---------------------------------------------------------------------------
// BytesTrie (exact bytestrie.cpp node arithmetic)
// ---------------------------------------------------------------------------
const BT_MAX_BRANCH_LINEAR: i32 = 5;
const BT_MIN_LINEAR_MATCH: u8 = 0x10;
const BT_MIN_VALUE_LEAD: u8 = 0x20;
const BT_VALUE_IS_FINAL: u8 = 1;
const BT_MIN_ONE_BYTE_VALUE_LEAD: u8 = 0x10;
const BT_MIN_TWO_BYTE_VALUE_LEAD: u8 = 0x51;
const BT_MIN_THREE_BYTE_VALUE_LEAD: u8 = 0x6c;
const BT_FOUR_BYTE_VALUE_LEAD: u8 = 0x7e;
const BT_MIN_TWO_BYTE_DELTA_LEAD: u8 = 0xc0;
const BT_MIN_THREE_BYTE_DELTA_LEAD: u8 = 0xf0;
const BT_FOUR_BYTE_DELTA_LEAD: u8 = 0xfe;

fn value_result_b(node: u8) -> i32 {
    INTERMEDIATE_VALUE - (node & BT_VALUE_IS_FINAL) as i32
}
fn read_value_b(b: &[u8], pos: usize, lead: u8) -> u32 {
    if lead < BT_MIN_TWO_BYTE_VALUE_LEAD {
        return lead.wrapping_sub(BT_MIN_ONE_BYTE_VALUE_LEAD) as u32;
    }
    if lead < BT_MIN_THREE_BYTE_VALUE_LEAD {
        return (((lead.wrapping_sub(BT_MIN_TWO_BYTE_VALUE_LEAD)) as u32) << 8) | b[pos] as u32;
    }
    if lead < BT_FOUR_BYTE_VALUE_LEAD {
        return (((lead.wrapping_sub(BT_MIN_THREE_BYTE_VALUE_LEAD)) as u32) << 16)
            | ((b[pos] as u32) << 8)
            | b[pos + 1] as u32;
    }
    if lead == BT_FOUR_BYTE_VALUE_LEAD {
        return ((b[pos] as u32) << 16) | ((b[pos + 1] as u32) << 8) | b[pos + 2] as u32;
    }
    ((b[pos] as u32) << 24) | ((b[pos + 1] as u32) << 16) | ((b[pos + 2] as u32) << 8) | b[pos + 3] as u32
}
fn skip_value_b(_b: &[u8], mut pos: usize, lead: u8) -> usize {
    if lead >= BT_MIN_TWO_BYTE_VALUE_LEAD << 1 {
        if lead < BT_MIN_THREE_BYTE_VALUE_LEAD << 1 {
            pos += 1;
        } else if lead < BT_FOUR_BYTE_VALUE_LEAD << 1 {
            pos += 2;
        } else {
            pos += 3 + ((lead >> 1) & 1) as usize;
        }
    }
    pos
}
fn skip_value_from_b(b: &[u8], mut pos: usize) -> usize {
    let lead = b[pos];
    pos += 1;
    skip_value_b(b, pos, lead)
}
fn jump_by_delta_b(b: &[u8], mut pos: usize) -> usize {
    let mut delta = b[pos] as u32;
    pos += 1;
    if delta < BT_MIN_TWO_BYTE_DELTA_LEAD as u32 {
        // one byte
    } else if delta < BT_MIN_THREE_BYTE_DELTA_LEAD as u32 {
        delta = ((delta - BT_MIN_TWO_BYTE_DELTA_LEAD as u32) << 8) | b[pos] as u32;
        pos += 1;
    } else if delta < BT_FOUR_BYTE_DELTA_LEAD as u32 {
        delta = ((delta - BT_MIN_THREE_BYTE_DELTA_LEAD as u32) << 16) | ((b[pos] as u32) << 8) | b[pos + 1] as u32;
        pos += 2;
    } else if delta == BT_FOUR_BYTE_DELTA_LEAD as u32 {
        delta = ((b[pos] as u32) << 16) | ((b[pos + 1] as u32) << 8) | b[pos + 2] as u32;
        pos += 3;
    } else {
        delta = ((b[pos] as u32) << 24)
            | ((b[pos + 1] as u32) << 16)
            | ((b[pos + 2] as u32) << 8)
            | b[pos + 3] as u32;
        pos += 4;
    }
    pos + delta as usize
}
fn skip_delta_b(b: &[u8], mut pos: usize) -> usize {
    let delta = b[pos];
    pos += 1;
    if delta >= BT_MIN_TWO_BYTE_DELTA_LEAD {
        if delta < BT_MIN_THREE_BYTE_DELTA_LEAD {
            pos += 1;
        } else if delta < BT_FOUR_BYTE_DELTA_LEAD {
            pos += 2;
        } else {
            pos += 3 + (delta & 1) as usize;
        }
    }
    pos
}

struct BytesTrie<'a> {
    b: &'a [u8],
    pos: i64,
    remaining: i32,
}

impl<'a> BytesTrie<'a> {
    fn new(b: &'a [u8]) -> Self {
        BytesTrie { b, pos: 0, remaining: -1 }
    }
    fn stop(&mut self) {
        self.pos = -1;
        self.remaining = -1;
    }
    fn first(&mut self, in_byte: i32) -> i32 {
        self.remaining = -1;
        self.next(in_byte)
    }
    fn next(&mut self, in_byte: i32) -> i32 {
        if self.pos < 0 {
            return NO_MATCH;
        }
        let length = self.remaining;
        if length >= 0 {
            if in_byte >= 0 && self.b[self.pos as usize] as i32 == in_byte {
                self.remaining = length - 1;
                self.pos += 1;
                if length - 1 < 0 && self.b[self.pos as usize] >= BT_MIN_VALUE_LEAD {
                    return value_result_b(self.b[self.pos as usize]);
                }
                return NO_VALUE;
            }
            self.stop();
            return NO_MATCH;
        }
        self.next_impl(self.pos, in_byte)
    }
    fn next_impl(&mut self, mut pos: i64, in_byte: i32) -> i32 {
        let b = self.b;
        loop {
            let node = b[pos as usize];
            pos += 1;
            if node < BT_MIN_LINEAR_MATCH {
                return self.branch_next(pos, node as i32, in_byte);
            } else if node < BT_MIN_VALUE_LEAD {
                let length = node as i32 - BT_MIN_LINEAR_MATCH as i32;
                if in_byte >= 0 && b[pos as usize] as i32 == in_byte {
                    pos += 1;
                    self.remaining = length - 1;
                    self.pos = pos;
                    if length - 1 < 0 && b[pos as usize] >= BT_MIN_VALUE_LEAD {
                        return value_result_b(b[pos as usize]);
                    }
                    return NO_VALUE;
                }
                break;
            } else if node & BT_VALUE_IS_FINAL != 0 {
                break;
            } else {
                pos = skip_value_b(b, pos as usize, node) as i64;
            }
        }
        self.stop();
        NO_MATCH
    }
    fn branch_next(&mut self, mut pos: i64, mut length: i32, in_byte: i32) -> i32 {
        let b = self.b;
        if length == 0 {
            length = b[pos as usize] as i32;
            pos += 1;
        }
        length += 1;
        while length > BT_MAX_BRANCH_LINEAR {
            let less = in_byte < b[pos as usize] as i32;
            pos += 1;
            if less {
                length >>= 1;
                pos = jump_by_delta_b(b, pos as usize) as i64;
            } else {
                length = length - (length >> 1);
                pos = skip_delta_b(b, pos as usize) as i64;
            }
        }
        loop {
            if in_byte == b[pos as usize] as i32 {
                pos += 1;
                let mut node = b[pos as usize];
                let result;
                if node & BT_VALUE_IS_FINAL != 0 {
                    result = FINAL_VALUE;
                } else {
                    pos += 1;
                    let lead = node >> 1;
                    let delta: u32;
                    if lead < BT_MIN_TWO_BYTE_VALUE_LEAD {
                        delta = lead.wrapping_sub(BT_MIN_ONE_BYTE_VALUE_LEAD) as u32;
                    } else if lead < BT_MIN_THREE_BYTE_VALUE_LEAD {
                        delta = ((lead.wrapping_sub(BT_MIN_TWO_BYTE_VALUE_LEAD) as u32) << 8)
                            | b[pos as usize] as u32;
                        pos += 1;
                    } else if lead < BT_FOUR_BYTE_VALUE_LEAD {
                        delta = ((lead.wrapping_sub(BT_MIN_THREE_BYTE_VALUE_LEAD) as u32) << 16)
                            | ((b[pos as usize] as u32) << 8)
                            | b[pos as usize + 1] as u32;
                        pos += 2;
                    } else if lead == BT_FOUR_BYTE_VALUE_LEAD {
                        delta = ((b[pos as usize] as u32) << 16)
                            | ((b[pos as usize + 1] as u32) << 8)
                            | b[pos as usize + 2] as u32;
                        pos += 3;
                    } else {
                        delta = ((b[pos as usize] as u32) << 24)
                            | ((b[pos as usize + 1] as u32) << 16)
                            | ((b[pos as usize + 2] as u32) << 8)
                            | b[pos as usize + 3] as u32;
                        pos += 4;
                    }
                    pos += delta as i64;
                    node = b[pos as usize];
                    result = if node >= BT_MIN_VALUE_LEAD { value_result_b(node) } else { NO_VALUE };
                }
                self.pos = pos;
                return result;
            }
            pos += 1;
            length -= 1;
            pos = skip_value_from_b(b, pos as usize) as i64;
            if length <= 1 {
                break;
            }
        }
        // do/while (length > 1): fall through to the final comparison
        if in_byte == b[pos as usize] as i32 {
            pos += 1;
            self.pos = pos;
            let node = b[pos as usize];
            return if node >= BT_MIN_VALUE_LEAD { value_result_b(node) } else { NO_VALUE };
        }
        self.stop();
        NO_MATCH
    }
    fn get_value(&self) -> u32 {
        let pos = self.pos as usize;
        let lead_byte = self.b[pos];
        read_value_b(self.b, pos + 1, lead_byte >> 1)
    }
}

// ---------------------------------------------------------------------------
// UCharsTrie (exact ucharstrie.cpp node arithmetic)
// ---------------------------------------------------------------------------
const UT_MAX_BRANCH_LINEAR: i32 = 5;
const UT_MIN_LINEAR_MATCH: u16 = 0x30;
const UT_MIN_VALUE_LEAD: u16 = 0x40;
const UT_NODE_TYPE_MASK: u16 = 0x3f;
const UT_VALUE_IS_FINAL: u16 = 0x8000;
const UT_MIN_TWO_UNIT_VALUE_LEAD: u16 = 0x4000;
const UT_THREE_UNIT_VALUE_LEAD: u16 = 0x7fff;
const UT_MIN_TWO_UNIT_NODE_VALUE_LEAD: u16 = 0x4040;
const UT_THREE_UNIT_NODE_VALUE_LEAD: u16 = 0x7fc0;
const UT_MIN_TWO_UNIT_DELTA_LEAD: u16 = 0xfc00;
const UT_THREE_UNIT_DELTA_LEAD: u16 = 0xffff;

fn value_result_u(node: u16) -> i32 {
    INTERMEDIATE_VALUE - (node >> 15) as i32
}
fn read_value_u(u: &[u16], pos: usize, lead: u16) -> u32 {
    if lead < UT_MIN_TWO_UNIT_VALUE_LEAD {
        return lead as u32;
    }
    if lead < UT_THREE_UNIT_VALUE_LEAD {
        return (((lead - UT_MIN_TWO_UNIT_VALUE_LEAD) as u32) << 16) | u[pos] as u32;
    }
    ((u[pos] as u32) << 16) | u[pos + 1] as u32
}
fn read_node_value_u(u: &[u16], pos: usize, lead: u16) -> u32 {
    if lead < UT_MIN_TWO_UNIT_NODE_VALUE_LEAD {
        return ((lead >> 6) - 1) as u32;
    }
    if lead < UT_THREE_UNIT_NODE_VALUE_LEAD {
        return ((((lead & 0x7fc0) - UT_MIN_TWO_UNIT_NODE_VALUE_LEAD) as u32) << 10) | u[pos] as u32;
    }
    ((u[pos] as u32) << 16) | u[pos + 1] as u32
}
fn skip_value_u(_u: &[u16], mut pos: usize, lead: u16) -> usize {
    if lead >= UT_MIN_TWO_UNIT_VALUE_LEAD {
        pos += if lead < UT_THREE_UNIT_VALUE_LEAD { 1 } else { 2 };
    }
    pos
}
fn skip_value_from_u(u: &[u16], mut pos: usize) -> usize {
    let lead = u[pos] & 0x7fff;
    pos += 1;
    skip_value_u(u, pos, lead)
}
fn skip_node_value_u(_u: &[u16], mut pos: usize, lead: u16) -> usize {
    if lead >= UT_MIN_TWO_UNIT_NODE_VALUE_LEAD {
        pos += if lead < UT_THREE_UNIT_NODE_VALUE_LEAD { 1 } else { 2 };
    }
    pos
}
fn jump_by_delta_u(u: &[u16], mut pos: usize) -> usize {
    let mut delta = u[pos] as u32;
    pos += 1;
    if delta >= UT_MIN_TWO_UNIT_DELTA_LEAD as u32 {
        if delta == UT_THREE_UNIT_DELTA_LEAD as u32 {
            delta = ((u[pos] as u32) << 16) | u[pos + 1] as u32;
            pos += 2;
        } else {
            delta = ((delta - UT_MIN_TWO_UNIT_DELTA_LEAD as u32) << 16) | u[pos] as u32;
            pos += 1;
        }
    }
    pos + delta as usize
}
fn skip_delta_u(u: &[u16], mut pos: usize) -> usize {
    let delta = u[pos];
    pos += 1;
    if delta >= UT_MIN_TWO_UNIT_DELTA_LEAD {
        pos += if delta == UT_THREE_UNIT_DELTA_LEAD { 2 } else { 1 };
    }
    pos
}

struct UCharsTrie<'a> {
    u: &'a [u16],
    pos: i64,
    remaining: i32,
}

impl<'a> UCharsTrie<'a> {
    fn new(u: &'a [u16]) -> Self {
        UCharsTrie { u, pos: 0, remaining: -1 }
    }
    fn stop(&mut self) {
        self.pos = -1;
        self.remaining = -1;
    }
    fn first(&mut self, uchar: i32) -> i32 {
        self.remaining = -1;
        self.next(uchar)
    }
    fn next(&mut self, uchar: i32) -> i32 {
        if self.pos < 0 {
            return NO_MATCH;
        }
        let length = self.remaining;
        if length >= 0 {
            if uchar >= 0 && self.u[self.pos as usize] as i32 == uchar {
                self.remaining = length - 1;
                self.pos += 1;
                if length - 1 < 0 && self.u[self.pos as usize] >= UT_MIN_VALUE_LEAD {
                    return value_result_u(self.u[self.pos as usize]);
                }
                return NO_VALUE;
            }
            self.stop();
            return NO_MATCH;
        }
        self.next_impl(self.pos, uchar)
    }
    fn next_impl(&mut self, mut pos: i64, uchar: i32) -> i32 {
        let u = self.u;
        let mut node = u[pos as usize];
        pos += 1;
        loop {
            if node < UT_MIN_LINEAR_MATCH {
                return self.branch_next(pos, node as i32, uchar);
            } else if node < UT_MIN_VALUE_LEAD {
                let length = node as i32 - UT_MIN_LINEAR_MATCH as i32;
                if uchar >= 0 && u[pos as usize] as i32 == uchar {
                    pos += 1;
                    self.remaining = length - 1;
                    self.pos = pos;
                    if length - 1 < 0 && u[pos as usize] >= UT_MIN_VALUE_LEAD {
                        return value_result_u(u[pos as usize]);
                    }
                    return NO_VALUE;
                }
                break;
            } else if node & UT_VALUE_IS_FINAL != 0 {
                break;
            } else {
                pos = skip_node_value_u(u, pos as usize, node) as i64;
                node &= UT_NODE_TYPE_MASK;
            }
        }
        self.stop();
        NO_MATCH
    }
    fn branch_next(&mut self, mut pos: i64, mut length: i32, uchar: i32) -> i32 {
        let u = self.u;
        if length == 0 {
            length = u[pos as usize] as i32;
            pos += 1;
        }
        length += 1;
        while length > UT_MAX_BRANCH_LINEAR {
            let less = uchar < u[pos as usize] as i32;
            pos += 1;
            if less {
                length >>= 1;
                pos = jump_by_delta_u(u, pos as usize) as i64;
            } else {
                length = length - (length >> 1);
                pos = skip_delta_u(u, pos as usize) as i64;
            }
        }
        loop {
            if uchar == u[pos as usize] as i32 {
                pos += 1;
                let mut node = u[pos as usize];
                let result;
                if node & UT_VALUE_IS_FINAL != 0 {
                    result = FINAL_VALUE;
                } else {
                    pos += 1;
                    let delta: u32;
                    if node < UT_MIN_TWO_UNIT_VALUE_LEAD {
                        delta = node as u32;
                    } else if node < UT_THREE_UNIT_VALUE_LEAD {
                        delta = ((node - UT_MIN_TWO_UNIT_VALUE_LEAD) as u32) << 16 | u[pos as usize] as u32;
                        pos += 1;
                    } else {
                        delta = ((u[pos as usize] as u32) << 16) | u[pos as usize + 1] as u32;
                        pos += 2;
                    }
                    pos += delta as i64;
                    node = u[pos as usize];
                    result = if node >= UT_MIN_VALUE_LEAD { value_result_u(node) } else { NO_VALUE };
                }
                self.pos = pos;
                return result;
            }
            pos += 1;
            length -= 1;
            pos = skip_value_from_u(u, pos as usize) as i64;
            if length <= 1 {
                break;
            }
        }
        if uchar == u[pos as usize] as i32 {
            pos += 1;
            self.pos = pos;
            let node = u[pos as usize];
            return if node >= UT_MIN_VALUE_LEAD { value_result_u(node) } else { NO_VALUE };
        }
        self.stop();
        NO_MATCH
    }
    fn get_value(&self) -> u32 {
        let pos = self.pos as usize;
        let lead = self.u[pos];
        if lead & UT_VALUE_IS_FINAL != 0 {
            read_value_u(self.u, pos + 1, lead & 0x7fff)
        } else {
            read_node_value_u(self.u, pos + 1, lead)
        }
    }
}

// ---------------------------------------------------------------------------
// Dictionary matchers (dictionarydata.cpp matches())
// ---------------------------------------------------------------------------

/// BytesDictionaryMatcher::matches() for SEA dicts (offset transform).
#[allow(clippy::too_many_arguments)]
fn bytes_matches(
    ut: &mut UText,
    dict: &[u8],
    dict_offset: u32,
    max_length: usize,
    limit: usize,
    mut lengths: Option<&mut [usize]>,
    mut cp_lengths: Option<&mut [usize]>,
    mut values: Option<&mut [u32]>,
    mut prefix: Option<&mut usize>,
) -> usize {
    let mut bt = BytesTrie::new(dict);
    let starting_text_index = ut.index();
    let mut word_count = 0usize;
    let mut code_points_matched = 0usize;
    let transform = |c: u32| -> i32 {
        if c == 0x200d {
            return 0xff;
        }
        if c == 0x200c {
            return 0xfe;
        }
        let delta = c as i64 - dict_offset as i64;
        if delta < 0 || delta > 0xfd {
            return -1;
        }
        delta as i32
    };
    loop {
        let c = ut.next32();
        if c < 0 {
            break;
        }
        let result = if code_points_matched == 0 {
            bt.first(transform(c as u32))
        } else {
            bt.next(transform(c as u32))
        };
        let length_matched = ut.index() - starting_text_index;
        code_points_matched += 1;
        if result & 2 != 0 {
            // HAS_VALUE
            if word_count < limit {
                if let Some(v) = values.as_deref_mut() {
                    v[word_count] = bt.get_value();
                }
                if let Some(l) = lengths.as_deref_mut() {
                    l[word_count] = length_matched;
                }
                if let Some(cl) = cp_lengths.as_deref_mut() {
                    cl[word_count] = code_points_matched;
                }
                word_count += 1;
            }
            if result == FINAL_VALUE {
                break;
            }
        } else if result == NO_MATCH {
            break;
        }
        if length_matched >= max_length {
            break;
        }
    }
    if let Some(p) = prefix.as_deref_mut() {
        *p = code_points_matched;
    }
    word_count
}

/// DictionaryMatcher::matches() for cjdict (UCharsTrie, raw code points).
#[allow(clippy::too_many_arguments)]
fn uchars_matches(
    ut: &mut UText,
    dict_units: &[u16],
    max_length: usize,
    limit: usize,
    mut lengths: Option<&mut [usize]>,
    mut cp_lengths: Option<&mut [usize]>,
    mut values: Option<&mut [u32]>,
    mut prefix: Option<&mut usize>,
) -> usize {
    let mut uct = UCharsTrie::new(dict_units);
    let starting_text_index = ut.index();
    let mut word_count = 0usize;
    let mut code_points_matched = 0usize;
    loop {
        let c = ut.next32();
        if c < 0 {
            break;
        }
        let result = if code_points_matched == 0 {
            uct.first(c)
        } else {
            uct.next(c)
        };
        let length_matched = ut.index() - starting_text_index;
        code_points_matched += 1;
        if result & 2 != 0 {
            // HAS_VALUE
            if word_count < limit {
                if let Some(v) = values.as_deref_mut() {
                    v[word_count] = uct.get_value();
                }
                if let Some(l) = lengths.as_deref_mut() {
                    l[word_count] = length_matched;
                }
                if let Some(cl) = cp_lengths.as_deref_mut() {
                    cl[word_count] = code_points_matched;
                }
                word_count += 1;
            }
            if result == FINAL_VALUE {
                break;
            }
        } else if result == NO_MATCH {
            break;
        }
        if length_matched >= max_length {
            break;
        }
    }
    if let Some(p) = prefix.as_deref_mut() {
        *p = code_points_matched;
    }
    word_count
}

fn cj_dict_units() -> &'static Vec<u16> {
    static UNITS: OnceLock<Vec<u16>> = OnceLock::new();
    UNITS.get_or_init(|| {
        CJ_DICT_TRIE_BYTES
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect()
    })
}

// ---------------------------------------------------------------------------
// RBBI handleNext (rbbi.cpp) — forward state machine over word.brk
// ---------------------------------------------------------------------------
const START_STATE: usize = 1;
const STOP_STATE: usize = 0;

fn rbbi_row(state: usize) -> &'static [u8] {
    &RBBI_ROWS[state * RBBI_ROW_LEN..(state + 1) * RBBI_ROW_LEN]
}

struct HandleNextResult {
    pos: usize,
    status_idx: usize,
    dict_count: u32,
}

fn handle_next(text: &mut UText, initial_position: usize) -> Option<HandleNextResult> {
    let mut status_idx = 0usize;
    let mut dict_count = 0u32;
    let dict_start = RBBI_DICT_CATEGORIES_START as usize;

    text.set_index(initial_position);
    let mut result = initial_position;
    let mut c = text.next32();
    if c < 0 {
        return None; // done
    }

    let mut state = START_STATE;
    let mut row = rbbi_row(state);
    let mut mode = 0u8; // RBBI_RUN=0 (word.brk has no BOF → mode never becomes 1)
    let mut category: usize;

    loop {
        if c < 0 {
            if mode == 2 {
                break;
            }
            mode = 2;
            category = 1; // EOF pseudo-category
        } else if mode == 0 {
            category = category_of(c as u32) as usize;
            if category >= dict_start {
                dict_count += 1;
            }
        } else {
            // mode == 2 with c >= 0 is unreachable (mode only set when c < 0,
            // and c is not advanced afterwards); keep the last category.
            category = 1;
        }

        state = row[3 + category] as usize;
        row = rbbi_row(state);

        let accepting = row[0];
        if accepting == 1 {
            // mode is never 1 (no BOF)
            result = text.index();
            status_idx = row[2] as usize;
        } else if accepting > 1 {
            unreachable!("unexpected lookahead state in word.brk");
        }
        if row[1] > 1 {
            unreachable!("unexpected lookahead rule in word.brk");
        }

        if state == STOP_STATE {
            break;
        }

        if mode == 0 {
            c = text.next32();
        }
    }

    if result == initial_position {
        // failed to advance; force ahead by one code point
        text.set_index(initial_position);
        text.next32();
        result = text.index();
        status_idx = 0;
    }
    Some(HandleNextResult { pos: result, status_idx, dict_count })
}

// ---------------------------------------------------------------------------
// PossibleWord (dictbe.cpp)
// ---------------------------------------------------------------------------
const POSSIBLE_WORD_LIST_MAX: usize = 20;

struct PossibleWord {
    count: usize,
    prefix: usize,
    offset: i64,
    mark: i64,
    current: i64,
    cu_lengths: [usize; POSSIBLE_WORD_LIST_MAX],
    cp_lengths: [usize; POSSIBLE_WORD_LIST_MAX],
}

impl PossibleWord {
    fn new() -> Self {
        PossibleWord {
            count: 0,
            prefix: 0,
            offset: -1,
            mark: 0,
            current: 0,
            cu_lengths: [0; POSSIBLE_WORD_LIST_MAX],
            cp_lengths: [0; POSSIBLE_WORD_LIST_MAX],
        }
    }
    fn candidates(&mut self, text: &mut UText, dict: &[u8], dict_offset: u32, range_end: usize) -> usize {
        let start = text.index();
        if start as i64 != self.offset {
            self.offset = start as i64;
            let mut prefix = 0usize;
            self.count = bytes_matches(
                text,
                dict,
                dict_offset,
                range_end - start,
                POSSIBLE_WORD_LIST_MAX,
                Some(&mut self.cu_lengths[..]),
                Some(&mut self.cp_lengths[..]),
                None,
                Some(&mut prefix),
            );
            self.prefix = prefix;
            if self.count == 0 {
                text.set_index(start);
            }
        }
        if self.count > 0 {
            text.set_index(start + self.cu_lengths[self.count - 1]);
        }
        self.current = self.count as i64 - 1;
        self.mark = self.current;
        self.count
    }
    fn accept_marked(&self, text: &mut UText) -> usize {
        text.set_index(self.offset as usize + self.cu_lengths[self.mark as usize]);
        self.cu_lengths[self.mark as usize]
    }
    fn back_up(&mut self, text: &mut UText) -> bool {
        if self.current > 0 {
            self.current -= 1;
            text.set_index(self.offset as usize + self.cu_lengths[self.current as usize]);
            true
        } else {
            false
        }
    }
    fn longest_prefix(&self) -> usize {
        self.prefix
    }
    fn mark_current(&mut self) {
        self.mark = self.current;
    }
    fn marked_cp_length(&self) -> usize {
        self.cp_lengths[self.mark as usize]
    }
}

// ---------------------------------------------------------------------------
// SEA engines: Thai/Lao/Khmer/Burmese (dictbe.cpp)
// ---------------------------------------------------------------------------
struct SeaEngineCfg {
    word_set: &'static [(u32, u32)],
    end_word_set: &'static [(u32, u32)],
    begin_word_set: &'static [(u32, u32)],
    mark_set: &'static [(u32, u32)],
    suffix_set: &'static [(u32, u32)],
    dict: &'static [u8],
    dict_offset: u32,
    is_thai: bool,
}

static THAI_ENGINE: SeaEngineCfg = SeaEngineCfg {
    word_set: THAI_WORD_SET,
    end_word_set: THAI_END_WORD_SET,
    begin_word_set: THAI_BEGIN_WORD_SET,
    mark_set: THAI_MARK_SET,
    suffix_set: THAI_SUFFIX_SET,
    dict: THAI_DICT_TRIE,
    dict_offset: THAI_DICT_OFFSET,
    is_thai: true,
};
static LAO_ENGINE: SeaEngineCfg = SeaEngineCfg {
    word_set: LAO_WORD_SET,
    end_word_set: LAO_END_WORD_SET,
    begin_word_set: LAO_BEGIN_WORD_SET,
    mark_set: LAO_MARK_SET,
    suffix_set: &[],
    dict: LAO_DICT_TRIE,
    dict_offset: LAO_DICT_OFFSET,
    is_thai: false,
};
static KHMER_ENGINE: SeaEngineCfg = SeaEngineCfg {
    word_set: KHMER_WORD_SET,
    end_word_set: KHMER_END_WORD_SET,
    begin_word_set: KHMER_BEGIN_WORD_SET,
    mark_set: KHMER_MARK_SET,
    suffix_set: &[],
    dict: KHMER_DICT_TRIE,
    dict_offset: KHMER_DICT_OFFSET,
    is_thai: false,
};
static BURMESE_ENGINE: SeaEngineCfg = SeaEngineCfg {
    word_set: BURMESE_WORD_SET,
    end_word_set: BURMESE_END_WORD_SET,
    begin_word_set: BURMESE_BEGIN_WORD_SET,
    mark_set: BURMESE_MARK_SET,
    suffix_set: &[],
    dict: BURMESE_DICT_TRIE,
    dict_offset: BURMESE_DICT_OFFSET,
    is_thai: false,
};

/// dictbe.cpp: LOOKAHEAD=3, ROOT/PREFIX_COMBINE_THRESHOLD=3, MIN_WORD=2,
/// MIN_WORD_SPAN=4 — but only Thai uses code-point semantics for the span
/// check (utext_moveIndex32); Lao/Khmer/Burmese compare code units.
fn sea_divide_up_dictionary_range(
    text: &mut UText,
    range_start: usize,
    range_end: usize,
    found_breaks: &mut Vec<usize>,
    eng: &SeaEngineCfg,
) -> i64 {
    const MIN_WORD_SPAN: i64 = 4;
    if eng.is_thai {
        // Thai (dictbe.cpp:241): move 4 CODE POINTS, bail if that reaches rangeEnd
        text.set_index(range_start);
        text.move_index32(MIN_WORD_SPAN as i32);
        if text.index() as i64 >= range_end as i64 {
            return 0; // Not enough characters for two words
        }
    } else if ((range_end - range_start) as i64) < MIN_WORD_SPAN {
        // Lao/Khmer/Burmese (dictbe.cpp:480/879/672): CODE UNIT arithmetic
        return 0;
    }
    text.set_index(range_start);

    let mut words_found: i64 = 0;
    let mut cp_word_length: usize;
    let mut cu_word_length: usize;
    let mut words = [PossibleWord::new(), PossibleWord::new(), PossibleWord::new()];

    text.set_index(range_start);
    loop {
        let current = text.index();
        if current >= range_end {
            break;
        }
        cp_word_length = 0;
        cu_word_length = 0;

        let candidates = words[(words_found % 3) as usize].candidates(text, eng.dict, eng.dict_offset, range_end);

        if candidates == 1 {
            cu_word_length = words[(words_found % 3) as usize].accept_marked(text);
            cp_word_length = words[(words_found % 3) as usize].marked_cp_length();
            words_found += 1;
        } else if candidates > 1 {
            // If we're already at the end of the range, we're done
            'found_best: {
                if text.index() >= range_end {
                    break 'found_best;
                }
                loop {
                    if words[((words_found + 1) % 3) as usize].candidates(text, eng.dict, eng.dict_offset, range_end) > 0 {
                        words[(words_found % 3) as usize].mark_current();
                        if text.index() >= range_end {
                            break 'found_best;
                        }
                        loop {
                            if words[((words_found + 2) % 3) as usize].candidates(text, eng.dict, eng.dict_offset, range_end) != 0 {
                                words[(words_found % 3) as usize].mark_current();
                                break 'found_best;
                            }
                            if !words[((words_found + 1) % 3) as usize].back_up(text) {
                                break;
                            }
                        }
                    }
                    if !words[(words_found % 3) as usize].back_up(text) {
                        break;
                    }
                }
            }
            cu_word_length = words[(words_found % 3) as usize].accept_marked(text);
            cp_word_length = words[(words_found % 3) as usize].marked_cp_length();
            words_found += 1;
        }

        #[allow(unused_assignments)]
        let mut uc: i32 = 0;
        if text.index() < range_end && cp_word_length < 3 {
            if words[(words_found % 3) as usize].candidates(text, eng.dict, eng.dict_offset, range_end) == 0
                && (cu_word_length == 0 || words[(words_found % 3) as usize].longest_prefix() < 3)
            {
                // Look for a plausible word boundary
                let mut remaining: i64 = range_end as i64 - (current as i64 + cu_word_length as i64);
                let mut chars: usize = 0;
                loop {
                    let pc_index = text.index();
                    let pc = text.next32();
                    let pc_size = text.index() - pc_index;
                    chars += pc_size;
                    remaining -= pc_size as i64;
                    if remaining <= 0 {
                        break;
                    }
                    uc = text.current32();
                    if pc >= 0 && in_ranges(eng.end_word_set, pc as u32) && in_ranges(eng.begin_word_set, uc as u32) {
                        let num_candidates = words[((words_found + 1) % 3) as usize]
                            .candidates(text, eng.dict, eng.dict_offset, range_end);
                        text.set_index(current + cu_word_length + chars);
                        if num_candidates > 0 {
                            break;
                        }
                    }
                }
                if cu_word_length == 0 {
                    words_found += 1;
                }
                cu_word_length += chars;
            } else {
                text.set_index(current + cu_word_length);
            }
        }

        // Never stop before a combining mark.
        while text.index() < range_end && in_ranges(eng.mark_set, text.current32() as u32) {
            let curr_pos = text.index();
            text.next32();
            cu_word_length += text.index() - curr_pos;
        }

        // Thai suffix block (PAIYANNOI / MAIYAMOK)
        if eng.is_thai && text.index() < range_end && cu_word_length > 0 {
            uc = text.current32();
            if words[(words_found % 3) as usize].candidates(text, eng.dict, eng.dict_offset, range_end) == 0
                && in_ranges(eng.suffix_set, uc as u32)
            {
                if uc == 0x0e2f {
                    // PAIYANNOI
                    if !in_ranges(eng.suffix_set, text.previous32() as u32) {
                        text.next32();
                        let paiyannoi_index = text.index();
                        text.next32();
                        cu_word_length += text.index() - paiyannoi_index;
                        uc = text.current32();
                    } else {
                        text.next32();
                    }
                }
                if uc == 0x0e46 {
                    // MAIYAMOK
                    if text.previous32() != 0x0e46 {
                        text.next32();
                        let maiyamok_index = text.index();
                        text.next32();
                        cu_word_length += text.index() - maiyamok_index;
                    } else {
                        text.next32();
                    }
                }
            } else {
                text.set_index(current + cu_word_length);
            }
        }

        if cu_word_length > 0 {
            found_breaks.push(current + cu_word_length);
        }
    }

    if !found_breaks.is_empty() && *found_breaks.last().unwrap() >= range_end {
        found_breaks.pop();
        words_found -= 1;
    }
    words_found
}

// ---------------------------------------------------------------------------
// CjkBreakEngine::divideUpDictionaryRange (dictbe.cpp) — Viterbi + NFKC
// ---------------------------------------------------------------------------
const KUINT32MAX: u32 = u32::MAX;
const MAX_SNLP: u32 = 255;
const MAX_KATAKANA_LENGTH: usize = 8;
const MAX_KATAKANA_GROUP_LENGTH: usize = 20;
const KATAKANA_COST: [u32; 9] = [8192, 984, 408, 240, 204, 252, 300, 372, 480];

fn get_katakana_cost(word_length: usize) -> u32 {
    if word_length > MAX_KATAKANA_LENGTH {
        8192
    } else {
        KATAKANA_COST[word_length]
    }
}
fn is_katakana(v: u32) -> bool {
    ((0x30a1..=0x30fe).contains(&v) && v != 0x30fb) || (0xff66..=0xff9f).contains(&v)
}

fn cjk_divide_up_dictionary_range(
    text: &mut UText,
    range_start: usize,
    range_end: usize,
    found_breaks: &mut Vec<usize>,
) -> i64 {
    if range_start >= range_end {
        return 0;
    }

    // Contiguous-chunk fast path (UText over UTF-16 is always one chunk):
    let mut in_string: Vec<u16> = text.s[range_start..range_end].to_vec();
    let mut input_map: Option<Vec<usize>> = None; // None → 1:1 with rangeStart offset

    if nfkc(&in_string) != in_string {
        let mut normalized_parts: Vec<Vec<u16>> = Vec::new();
        let mut normalized_length = 0usize;
        let mut normalized_map: Vec<usize> = Vec::new();
        let mut src_i = 0usize;
        while src_i < in_string.len() {
            let fragment_start_i = src_i;
            let mut c = cp_at(&in_string, src_i);
            let mut fragment: Vec<u32> = Vec::new();
            loop {
                fragment.push(c as u32);
                src_i += cp_size(c);
                if src_i == in_string.len() {
                    break;
                }
                c = cp_at(&in_string, src_i);
                if has_boundary_before(c as u32) {
                    break;
                }
            }
            let mut frag16: Vec<u16> = Vec::with_capacity(fragment.len());
            encode_utf16(&fragment, &mut frag16);
            let nf = nfkc(&frag16);
            normalized_length += nf.len();
            let fragment_original_start = match &input_map {
                Some(m) => m[fragment_start_i],
                None => fragment_start_i + range_start,
            };
            while normalized_map.len() < normalized_length {
                normalized_map.push(fragment_original_start);
            }
            normalized_parts.push(nf);
        }
        let native_end = match &input_map {
            Some(m) => m[in_string.len()],
            None => in_string.len() + range_start,
        };
        normalized_map.push(native_end);
        input_map = Some(normalized_map);
        in_string = normalized_parts.concat();
    }

    let mut num_code_pts = 0usize;
    {
        let mut i = 0usize;
        while i < in_string.len() {
            num_code_pts += 1;
            i += cp_size(cp_at(&in_string, i));
        }
    }
    if num_code_pts != in_string.len() {
        // Supplementary characters: convert inputMap to code-point indexing
        let had_existing_map = input_map.is_some();
        let mut new_map: Vec<usize> = Vec::new();
        let mut cu_idx = 0usize;
        loop {
            if had_existing_map {
                new_map.push(input_map.as_ref().unwrap()[cu_idx]);
            } else {
                new_map.push(cu_idx + range_start);
            }
            // (JS assigns via newMap[cpIdx] = ...; push makes the index dead)
            if cu_idx == in_string.len() {
                break;
            }
            cu_idx += cp_size(cp_at(&in_string, cu_idx));
        }
        input_map = Some(new_map);
    }

    // bestSnlp[i] = snlp of best segmentation of first i code points
    let mut best_snlp = vec![KUINT32MAX; num_code_pts + 1];
    best_snlp[0] = 0;
    let mut prev: Vec<i64> = vec![-1; num_code_pts + 1];

    let max_word_size = 20usize;
    let mut values = vec![0u32; num_code_pts];
    let mut lengths = vec![0usize; num_code_pts];

    // Viterbi DP
    let mut ix = 0usize;
    let mut is_prev_katakana = false;
    let mut i = 0usize;
    while i < num_code_pts {
        if best_snlp[i] != KUINT32MAX {
            let mut fu = UText::new(&in_string);
            fu.set_index(ix);
            let mut count = uchars_matches(
                &mut fu,
                cj_dict_units(),
                max_word_size,
                num_code_pts,
                None,
                Some(&mut lengths[..]),
                Some(&mut values[..]),
                None,
            );

            // if there are no single character matches found in the dictionary
            // starting with this character, treat character as a 1-character
            // word with the highest value possible. Exclude Hangul.
            if (count == 0 || lengths[0] != 1) && !in_ranges(HANGUL_SET, cp_at(&in_string, ix) as u32) {
                // UVector32::setElementAt is a no-op out of bounds — replicate
                // exactly (out-of-bounds entries read as "undefined" in JS and
                // produce NaN comparisons, i.e. no DP update; see loop below).
                if count < values.len() {
                    values[count] = MAX_SNLP;
                }
                if count < lengths.len() {
                    lengths[count] = 1;
                }
                count += 1;
            }

            for j in 0..count {
                if j >= values.len() || j >= lengths.len() {
                    continue; // JS reads undefined → NaN → comparison false
                }
                let new_snlp = best_snlp[i].wrapping_add(values[j]);
                let lnji = lengths[j] + i;
                if new_snlp < best_snlp[lnji] {
                    best_snlp[lnji] = new_snlp;
                    prev[lnji] = i as i64;
                }
            }

            let katakana_here = is_katakana(cp_at(&in_string, ix) as u32);
            if !is_prev_katakana && katakana_here {
                let mut katakana_run_length = 1usize;
                let mut j = ix + cp_size(cp_at(&in_string, ix));
                while j < in_string.len()
                    && katakana_run_length < MAX_KATAKANA_GROUP_LENGTH
                    && is_katakana(cp_at(&in_string, j) as u32)
                {
                    j += cp_size(cp_at(&in_string, j));
                    katakana_run_length += 1;
                }
                if katakana_run_length < MAX_KATAKANA_GROUP_LENGTH {
                    let new_snlp = best_snlp[i].wrapping_add(get_katakana_cost(katakana_run_length));
                    if new_snlp < best_snlp[i + katakana_run_length] {
                        best_snlp[i + katakana_run_length] = new_snlp;
                        prev[i + katakana_run_length] = i as i64;
                    }
                }
            }
            is_prev_katakana = katakana_here;
        }
        // JS for-loop update expressions (run even for skipped iterations):
        i += 1;
        ix += cp_size(cp_at(&in_string, ix));
    }

    // Backtrack
    let mut t_boundary: Vec<usize> = Vec::new();
    if best_snlp[num_code_pts] == KUINT32MAX {
        t_boundary.push(num_code_pts);
    } else {
        let mut i = num_code_pts;
        while i > 0 {
            t_boundary.push(i);
            i = prev[i] as usize;
        }
    }
    if found_breaks.is_empty() || *found_breaks.last().unwrap() < range_start {
        t_boundary.push(0);
    }
    let num_breaks = t_boundary.len();

    // Convert positions back to original text indices
    let mut prev_utext_pos: i64 = -1;
    let mut corrected_num_breaks: i64 = 0;
    for i in (0..num_breaks).rev() {
        let cp_pos = t_boundary[i];
        let utext_pos = match &input_map {
            Some(m) => m[cp_pos],
            None => cp_pos + range_start,
        };
        if utext_pos as i64 > prev_utext_pos {
            // (isPhraseBreaking=false)
            if utext_pos != range_start {
                found_breaks.push(utext_pos);
                corrected_num_breaks += 1;
            }
        }
        // else: duplicate position (JS numBreaks-- is dead — the loop bound
        // was captured at entry and numBreaks is not used afterwards)
        prev_utext_pos = utext_pos as i64;
    }

    if !found_breaks.is_empty() && *found_breaks.last().unwrap() == range_end {
        found_breaks.pop();
        corrected_num_breaks -= 1;
    }
    corrected_num_breaks
}

// ---------------------------------------------------------------------------
// Language break engine dispatch
// (rbbi.cpp getLanguageBreakEngine + brkeng.cpp ICULanguageBreakFactory,
//  UnhandledEngine, loadEngineFor) — process-global factory cache semantics.
// ---------------------------------------------------------------------------
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum EngineKind {
    Thai,
    Lao,
    Khmer,
    Burmese,
    Cjk,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum EngineRef {
    Kind(EngineKind),
    Unhandled,
}

fn handles_kind(kind: EngineKind, c: u32) -> bool {
    match kind {
        EngineKind::Thai => in_ranges(THAI_WORD_SET, c),
        EngineKind::Lao => in_ranges(LAO_WORD_SET, c),
        EngineKind::Khmer => in_ranges(KHMER_WORD_SET, c),
        EngineKind::Burmese => in_ranges(BURMESE_WORD_SET, c),
        EngineKind::Cjk => in_ranges(CJ_SET, c),
    }
}

fn sea_cfg(kind: EngineKind) -> &'static SeaEngineCfg {
    match kind {
        EngineKind::Thai => &THAI_ENGINE,
        EngineKind::Lao => &LAO_ENGINE,
        EngineKind::Khmer => &KHMER_ENGINE,
        EngineKind::Burmese => &BURMESE_ENGINE,
        EngineKind::Cjk => unreachable!("Cjk is not a SEA engine"),
    }
}

// ICULanguageBreakFactory::fEngines — process-global list, iterated in
// REVERSE. Engines are created lazily by loadEngineFor and stay in the list
// for the lifetime of the process (module scope matches ICU's singleton).
static FACTORY_ENGINES: OnceLock<Mutex<Vec<EngineKind>>> = OnceLock::new();

// brkeng.cpp loadEngineFor: LSTM first (no LSTM .res in icudt78l.dat → always
// falls through), then loadDictionaryMatcherFor(script). Common ('Zyyy') and
// Hangul have no dictionary → no engine.
fn load_engine_for(c: u32) -> Option<EngineKind> {
    match script_of(c) {
        SCRIPT_THAI => Some(EngineKind::Thai),
        SCRIPT_LAO => Some(EngineKind::Lao),
        SCRIPT_KHMER => Some(EngineKind::Khmer),
        SCRIPT_MYANMAR => Some(EngineKind::Burmese),
        SCRIPT_HAN | SCRIPT_HIRAGANA | SCRIPT_KATAKANA => Some(EngineKind::Cjk),
        _ => None,
    }
}

// ICULanguageBreakFactory::getEngineFor: reverse iteration over the global
// engine list, then loadEngineFor creation (pushed to the global list).
fn factory_get_engine_for(c: u32) -> Option<EngineKind> {
    let list = FACTORY_ENGINES.get_or_init(|| Mutex::new(Vec::new()));
    let mut guard = list.lock().unwrap();
    for i in (0..guard.len()).rev() {
        if handles_kind(guard[i], c) {
            return Some(guard[i]);
        }
    }
    let eng = load_engine_for(c);
    if let Some(k) = eng {
        guard.push(k);
    }
    eng
}

// Per-BreakIterator engine state: each segment() call builds a fresh iterator
// in V8, so the instance engine stack + UnhandledEngine reject set are
// per-call while the factory cache stays process-global.
#[derive(Default)]
struct EngineCtx {
    engines: Vec<EngineRef>,
    unhandled_set: Option<Vec<(u32, u32)>>,
}

// rbbi.cpp RuleBasedBreakIterator::getLanguageBreakEngine:
//  1. instance engine list in REVERSE (UnhandledEngine sits at index 0, so it
//     is checked last — but still BEFORE the factory)
//  2. factory (process-global, reverse iteration, lazy creation)
//  3. factory null → reject the character's whole script via UnhandledEngine
fn get_language_break_engine(c: u32, ctx: &mut EngineCtx) -> EngineRef {
    for i in (0..ctx.engines.len()).rev() {
        let handled = match ctx.engines[i] {
            EngineRef::Kind(k) => handles_kind(k, c),
            EngineRef::Unhandled => ctx
                .unhandled_set
                .as_ref()
                .is_some_and(|s| in_ranges(s, c)),
        };
        if handled {
            return ctx.engines[i];
        }
    }
    if let Some(k) = factory_get_engine_for(c) {
        ctx.engines.push(EngineRef::Kind(k)); // fLanguageBreakEngines->push
        return EngineRef::Kind(k);
    }
    if ctx.unhandled_set.is_none() {
        ctx.unhandled_set = Some(Vec::new());
        ctx.engines.insert(0, EngineRef::Unhandled); // insertElementAt(0)
    }
    let set = ctx.unhandled_set.as_mut().unwrap();
    if !in_ranges(set, c) {
        // UnicodeSet::applyIntPropertyValue(UCHAR_SCRIPT, script) — replaces
        // the set with the entire script of c.
        *set = script_ranges_of(script_of(c));
    }
    EngineRef::Unhandled
}

// DictionaryBreakEngine::findBreaks (base, dictbe.cpp): span extension while
// the engine set contains the char, then divideUpDictionaryRange over the
// span. UnhandledEngine::findBreaks advances through the handled set and
// returns 0 breaks.
fn engine_find_breaks(
    eng: EngineRef,
    text: &mut UText,
    start_pos: usize,
    end_pos: usize,
    found_breaks: &mut Vec<usize>,
    ctx: &EngineCtx,
) -> i64 {
    let set: &[(u32, u32)] = match eng {
        EngineRef::Kind(EngineKind::Cjk) => CJ_SET,
        EngineRef::Kind(k) => sea_cfg(k).word_set,
        EngineRef::Unhandled => ctx.unhandled_set.as_ref().unwrap(),
    };
    text.set_index(start_pos);
    let mut ch = text.current32();
    let mut current;
    loop {
        current = text.index();
        if !(current < end_pos && in_ranges(set, ch as u32)) {
            break;
        }
        text.next32();
        ch = text.current32();
    }
    let n = match eng {
        EngineRef::Kind(EngineKind::Cjk) => cjk_divide_up_dictionary_range(text, start_pos, current, found_breaks),
        EngineRef::Kind(k) => {
            let cfg = sea_cfg(k);
            sea_divide_up_dictionary_range(text, start_pos, current, found_breaks, cfg)
        }
        EngineRef::Unhandled => 0,
    };
    text.set_index(current);
    n
}

// ---------------------------------------------------------------------------
// DictionaryCache (rbbi_cache.cpp) — forward-only semantics
// ---------------------------------------------------------------------------
struct DictionaryCache {
    breaks: Vec<usize>,
    start: usize,
    limit: usize,
    first_rule_status_index: usize,
    other_rule_status_index: usize,
}

impl DictionaryCache {
    fn new() -> Self {
        DictionaryCache {
            breaks: Vec::new(),
            start: 0,
            limit: 0,
            first_rule_status_index: 0,
            other_rule_status_index: 0,
        }
    }
    fn reset(&mut self) {
        self.breaks.clear();
        self.start = 0;
        self.limit = 0;
        self.first_rule_status_index = 0;
        self.other_rule_status_index = 0;
    }
    fn following(&self, from_pos: usize) -> Option<(usize, usize)> {
        if from_pos >= self.limit || from_pos < self.start {
            return None;
        }
        for &r in &self.breaks {
            if r > from_pos {
                return Some((r, self.other_rule_status_index));
            }
        }
        None // unreachable in ICU (UPRV_UNREACHABLE_EXIT)
    }
    fn populate(
        &mut self,
        start_pos: usize,
        end_pos: usize,
        first_rule_status: usize,
        other_rule_status: usize,
        text: &mut UText,
        ctx: &mut EngineCtx,
    ) {
        if end_pos - start_pos <= 1 {
            return; // keep stale cache (ICU does not reset)
        }
        self.reset();
        self.first_rule_status_index = first_rule_status;
        self.other_rule_status_index = other_rule_status;

        let range_end = end_pos;
        let mut found_break_count: i64 = 0;
        let dict_start = RBBI_DICT_CATEGORIES_START as usize;

        text.set_index(start_pos);
        let mut c = text.current32();
        let mut category = if c < 0 { 0 } else { category_of(c as u32) as usize };

        loop {
            while text.index() < range_end && category < dict_start {
                text.next32();
                c = text.current32();
                category = if c < 0 { 0 } else { category_of(c as u32) as usize };
            }
            if text.index() >= range_end {
                break;
            }

            // dictionary character at text.index(); dispatch engine
            let lbe = get_language_break_engine(c as u32, ctx);
            found_break_count +=
                engine_find_breaks(lbe, text, text.index(), range_end, &mut self.breaks, ctx);

            c = text.current32();
            category = if c < 0 { 0 } else { category_of(c as u32) as usize };
        }

        if found_break_count > 0 {
            if start_pos < self.breaks[0] {
                self.breaks.insert(0, start_pos);
            }
            if end_pos > *self.breaks.last().unwrap() {
                self.breaks.push(end_pos);
            }
            self.start = self.breaks[0];
            self.limit = *self.breaks.last().unwrap();
        }
    }
}

// ---------------------------------------------------------------------------
// Forward boundary iteration — the observable first()/current()/next() sequence
// ---------------------------------------------------------------------------
fn iterate_boundaries(s: &[u16]) -> Vec<(usize, usize)> {
    let mut text = UText::new(s);
    let mut dict = DictionaryCache::new();
    let mut ctx = EngineCtx::default();
    // first() → boundary 0 with status 0
    let mut out: Vec<(usize, usize)> = vec![(0, 0)];
    let mut pos = 0usize;
    let mut status_idx = 0usize;

    loop {
        // try dictionary cache first (populateFollowing)
        if let Some((r, si)) = dict.following(pos) {
            out.push((r, si));
            pos = r;
            continue;
        }
        let hn = match handle_next(&mut text, pos) {
            Some(h) => h,
            None => break,
        };
        if hn.dict_count > 0 {
            dict.populate(pos, hn.pos, status_idx, hn.status_idx, &mut text, &mut ctx);
            if let Some((r, si)) = dict.following(pos) {
                out.push((r, si));
                pos = r;
                continue;
            }
        }
        out.push((hn.pos, hn.status_idx));
        pos = hn.pos;
        status_idx = hn.status_idx;
    }
    out
}

// ---------------------------------------------------------------------------
// Public API — mirrors Intl.Segmenter word granularity
// ---------------------------------------------------------------------------
fn rule_status(status_idx: usize) -> i32 {
    let t = RBBI_STATUS;
    t[status_idx + t[status_idx] as usize]
}

/// V8 CurrentSegmentIsWordLike (js-segments.cc).
fn is_word_like_status(status: i32) -> bool {
    (status >= 100 && status < 300)
        || (status >= 200 && status < 500)
        || (status >= 400 && status < 700)
        || (status >= 600 && status < 700)
}

/// One `Intl.Segmenter` word-granularity segment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WordSegment {
    pub segment: String,
    pub is_word_like: bool,
}

/// `segmentWords(str)` — bit-exact port of Node's
/// `new Intl.Segmenter(undefined, { granularity: "word" }).segment(str)`
/// iteration (`segment` + `isWordLike` per segment).
pub fn segment_words(s: &str) -> Vec<WordSegment> {
    let units: Vec<u16> = s.encode_utf16().collect();
    let boundaries = iterate_boundaries(&units);
    let mut out = Vec::with_capacity(boundaries.len().saturating_sub(1));
    for w in boundaries.windows(2) {
        let start = w[0].0;
        let (end, status_idx) = w[1];
        let seg = String::from_utf16(&units[start..end])
            .expect("word-break boundaries are code-point aligned");
        out.push(WordSegment {
            segment: seg,
            is_word_like: is_word_like_status(rule_status(status_idx)),
        });
    }
    out
}

/// Word segments over raw UTF-16 units (may contain lone surrogates):
/// `(start, end, is_word_like)` per segment, UTF-16 code-unit indices.
pub(crate) fn segment_words_u16(units: &[u16]) -> Vec<(usize, usize, bool)> {
    let boundaries = iterate_boundaries(units);
    boundaries
        .windows(2)
        .map(|w| {
            let start = w[0].0;
            let (end, status_idx) = w[1];
            (start, end, is_word_like_status(rule_status(status_idx)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Binary fixture (generated by scripts/probe-word-seg.mjs --emit-fixture):
    // u32 case_count, then per case:
    //   u32 input_len, input_len × u16 (UTF-16 units, may contain lone surrogates)
    //   u32 seg_count, then per segment: u32 seg_len, seg_len × u16, u8 isWordLike
    // Fixture cases are fixed probes + seeded fuzz, all verified 0-fail against
    // live Intl.Segmenter (ICU 78.3) before emission.
    // -----------------------------------------------------------------------
    #[test]
    fn word_seg_fixture() {
        let data = include_bytes!("data/word-seg-fixture.bin");
        let u16_at = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
        let u32_at = |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);

        let mut p = 0usize;
        let case_count = u32_at(data, p) as usize;
        p += 4;
        assert!(case_count > 0, "empty fixture");

        let mut str_path_checked = 0usize;
        for ci in 0..case_count {
            let ilen = u32_at(data, p) as usize;
            p += 4;
            let units: Vec<u16> = (0..ilen).map(|k| u16_at(data, p + 2 * k)).collect();
            p += 2 * ilen;
            let nseg = u32_at(data, p) as usize;
            p += 4;
            let mut expected: Vec<(Vec<u16>, bool)> = Vec::with_capacity(nseg);
            for _ in 0..nseg {
                let slen = u32_at(data, p) as usize;
                p += 4;
                let seg: Vec<u16> = (0..slen).map(|k| u16_at(data, p + 2 * k)).collect();
                p += 2 * slen;
                let wl = data[p] != 0;
                p += 1;
                expected.push((seg, wl));
            }

            // u16-level path (covers lone surrogates)
            let boundaries = iterate_boundaries(&units);
            let mut actual: Vec<(Vec<u16>, bool)> = Vec::with_capacity(boundaries.len());
            for w in boundaries.windows(2) {
                let start = w[0].0;
                let (end, status_idx) = w[1];
                let word_like = is_word_like_status(rule_status(status_idx));
                actual.push((units[start..end].to_vec(), word_like));
            }
            assert_eq!(
                actual, expected,
                "case {ci}: units={:04x?}",
                units.iter().map(|u| format!("{u:04x}")).collect::<Vec<_>>()
            );

            // &str public API path when the input is valid UTF-16
            if let Ok(s) = String::from_utf16(&units) {
                let segs = segment_words(&s);
                let got: Vec<(Vec<u16>, bool)> = segs
                    .iter()
                    .map(|w| {
                        (
                            w.segment.encode_utf16().collect::<Vec<u16>>(),
                            w.is_word_like,
                        )
                    })
                    .collect();
                assert_eq!(got, expected, "case {ci} (&str path)");
                str_path_checked += 1;
            }
        }
        assert!(str_path_checked > case_count / 2, "suspiciously few valid-UTF-16 cases");
    }

    #[test]
    fn cjdict_trie_walks() {
        let u = cj_dict_units();
        let mut t = UCharsTrie::new(u);
        assert_eq!(t.first(0x4f60), INTERMEDIATE_VALUE); // 你
        assert_eq!(t.next(0x597d), INTERMEDIATE_VALUE); // 好
        assert_eq!(t.get_value(), 86);
        let mut t2 = UCharsTrie::new(u);
        assert_eq!(t2.first(0x4e16), INTERMEDIATE_VALUE); // 世
        assert_eq!(t2.next(0x754c), INTERMEDIATE_VALUE); // 界
        assert_eq!(t2.get_value(), 57);
    }

    #[test]
    fn known_dictionary_splits() {
        let repr = |s: &str| -> Vec<String> {
            segment_words(s)
                .iter()
                .map(|w| format!("{}{}", w.segment, if w.is_word_like { "*" } else { "-" }))
                .collect()
        };
        assert_eq!(
            repr("你好世界 test"),
            vec!["你好*", "世界*", " -", "test*"]
        );
        assert_eq!(repr("漢字テスト"), vec!["漢字*", "テスト*"]);
        assert_eq!(repr("カタカナ"), vec!["カタカナ*"]);
        // Lao: dictionary split ລາວ | ດີ
        let lao = repr("ລາວດີ");
        assert_eq!(lao.len(), 2);
        assert_eq!(lao[0], "ລາວ*");
        assert_eq!(lao[1], "ດີ*");
    }
}
