//! Port of `wheel-scroll.ts` (82 LOC) — wheel-event-to-line-count conversion
//! with velocity-based acceleration.
//!
//! The TS module-global `terminalAcceleratesWheel()` reads
//! `process.platform` and the SSH env vars once per `WheelScrollAccelerator`
//! construction (default argument). The Rust port injects the platform and
//! env via [`WheelScrollEnv`]: the app passes the real values, tests and the
//! parity harness inject fixed ones.

/// Lines moved per mouse-wheel event, or `"auto"` to accelerate fast wheel
/// spins (wheel-scroll.ts:2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WheelScrollLines {
    Lines(f64),
    Auto,
}

// Several events closer than this belong to one physical notch (Ghostty
// emits them ~4 ms apart) or come from a high-resolution source. They move
// one line each and do not accelerate.
const BURST_GAP_MS: f64 = 5.0;
// A pause longer than this ends a scroll gesture.
const GESTURE_GAP_MS: f64 = 200.0;
// Average event gap that maps to one line per event. Faster events scale up
// proportionally.
const REFERENCE_GAP_MS: f64 = 100.0;
const MAX_AUTO_LINES: f64 = 6.0;

/// The platform/env surface `terminalAcceleratesWheel()` reads
/// (wheel-scroll.ts:18-25).
pub struct WheelScrollEnv {
    /// `process.platform === "darwin"`.
    pub is_darwin: bool,
    /// `env.SSH_CONNECTION === undefined && env.SSH_CLIENT === undefined &&
    /// env.SSH_TTY === undefined`.
    pub ssh_free: bool,
}

impl WheelScrollEnv {
    /// Local macOS terminals receive wheel and trackpad deltas that the OS
    /// has already accelerated, and they emit one event per line. Other
    /// platforms, and SSH sessions where the client platform is unknown,
    /// usually send one event per wheel notch (wheel-scroll.ts:13-17).
    pub fn accelerates_wheel(&self) -> bool {
        self.is_darwin && self.ssh_free
    }
}

/// Converts wheel events into line counts (wheel-scroll.ts:35-74).
///
/// In `"auto"` mode on terminals that do not accelerate wheel input, the
/// count follows event velocity: an isolated notch moves one line, while a
/// fast spin moves up to six lines per event. For example, notches 100 ms
/// apart move 1 line each, 50 ms apart move 2, and 20 ms apart move 5.
pub struct WheelScrollAccelerator {
    lines: WheelScrollLines,
    accelerate: bool,
    last_time: f64,
    last_direction: i64,
    average_gap: Option<f64>,
    carry: f64,
}

impl WheelScrollAccelerator {
    /// `new WheelScrollAccelerator(lines, accelerate = !terminalAcceleratesWheel())`
    /// — pass `accelerate: None` to use the TS default
    /// `!env.accelerates_wheel()`.
    pub fn new(lines: WheelScrollLines, accelerate: Option<bool>, env: &WheelScrollEnv) -> Self {
        WheelScrollAccelerator {
            lines,
            accelerate: accelerate.unwrap_or_else(|| !env.accelerates_wheel()),
            // `Number.NEGATIVE_INFINITY`
            last_time: f64::NEG_INFINITY,
            last_direction: 0,
            average_gap: None,
            carry: 0.0,
        }
    }

    pub fn set_lines(&mut self, lines: WheelScrollLines) {
        self.lines = lines;
        self.reset();
    }

    /// Return the positive line count for a wheel event in `direction`
    /// (`-1` or `1`) at time `now` (milliseconds).
    pub fn next(&mut self, direction: i64, now: f64) -> f64 {
        match self.lines {
            // `Number.isFinite(this.lines) ? Math.max(1, Math.floor(this.lines)) : 1`
            WheelScrollLines::Lines(lines) => {
                return if lines.is_finite() { lines.floor().max(1.0) } else { 1.0 };
            }
            WheelScrollLines::Auto => {}
        }
        if !self.accelerate {
            return 1.0;
        }

        let gap = now - self.last_time;
        let same_gesture = direction == self.last_direction && gap <= GESTURE_GAP_MS;
        self.last_time = now;
        self.last_direction = direction;
        if !same_gesture {
            self.average_gap = None;
            self.carry = 0.0;
            return 1.0;
        }
        if gap < BURST_GAP_MS {
            return 1.0;
        }

        self.average_gap = Some(match self.average_gap {
            None => gap,
            Some(avg) => (avg + gap) / 2.0,
        });
        let lines = (REFERENCE_GAP_MS / self.average_gap.unwrap())
            .clamp(1.0, MAX_AUTO_LINES)
            + self.carry;
        let whole = lines.floor();
        self.carry = lines - whole;
        whole
    }

    fn reset(&mut self) {
        self.last_time = f64::NEG_INFINITY;
        self.last_direction = 0;
        self.average_gap = None;
        self.carry = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accelerator(lines: WheelScrollLines, accelerate: bool) -> WheelScrollAccelerator {
        WheelScrollAccelerator::new(
            lines,
            Some(accelerate),
            &WheelScrollEnv { is_darwin: false, ssh_free: true },
        )
    }

    #[test]
    fn fixed_lines_mode() {
        let mut a = accelerator(WheelScrollLines::Lines(3.0), true);
        assert_eq!(a.next(-1, 0.0), 3.0);
        assert_eq!(a.next(1, 10.0), 3.0);
        // Non-finite values fall back to 1 (JS `Number.isFinite` check).
        let mut b = accelerator(WheelScrollLines::Lines(f64::NAN), true);
        assert_eq!(b.next(1, 0.0), 1.0);
        // Fractional fixed lines floor to >= 1.
        let mut c = accelerator(WheelScrollLines::Lines(0.2), true);
        assert_eq!(c.next(1, 0.0), 1.0);
        // When not accelerating, auto still returns 1 per event.
        let mut d = accelerator(WheelScrollLines::Auto, false);
        assert_eq!(d.next(1, 0.0), 1.0);
        assert_eq!(d.next(1, 1.0), 1.0);
    }

    #[test]
    fn first_event_of_a_gesture_moves_one_line() {
        let mut a = accelerator(WheelScrollLines::Auto, true);
        assert_eq!(a.next(-1, 0.0), 1.0);
        // Direction change starts a new gesture.
        assert_eq!(a.next(1, 10.0), 1.0);
        // Pause longer than the gesture gap starts a new gesture.
        assert_eq!(a.next(1, 10.0 + 201.0), 1.0);
    }

    #[test]
    fn burst_events_do_not_accelerate() {
        let mut a = accelerator(WheelScrollLines::Auto, true);
        assert_eq!(a.next(1, 0.0), 1.0);
        // Events closer than BURST_GAP_MS (5ms) move one line each.
        assert_eq!(a.next(1, 4.0), 1.0);
        assert_eq!(a.next(1, 8.0), 1.0);
    }

    #[test]
    fn velocity_scales_line_counts() {
        let mut a = accelerator(WheelScrollLines::Auto, true);
        assert_eq!(a.next(1, 0.0), 1.0);
        // 50 ms average gap → REFERENCE_GAP/avg = 2 lines per event.
        assert_eq!(a.next(1, 50.0), 2.0);
        // 25 ms gap: average becomes 37.5 → 100/37.5 ≈ 2.67 → 2 whole lines,
        // carry 0.67.
        assert_eq!(a.next(1, 75.0), 2.0);
        // Next 25 ms gap: average 31.25 → 3.2 + carry 0.67 ≈ 3.87 → 3 lines.
        assert_eq!(a.next(1, 100.0), 3.0);
    }

    #[test]
    fn max_auto_lines_is_six() {
        let mut a = accelerator(WheelScrollLines::Auto, true);
        a.next(1, 0.0);
        // 5 ms is the burst threshold (gap < 5 returns 1); just above it the
        // reference gap caps the velocity at 6 lines.
        assert_eq!(a.next(1, 5.5), 6.0);
        assert_eq!(a.next(1, 11.0), 6.0);
    }

    #[test]
    fn set_lines_resets_the_gesture() {
        let mut a = accelerator(WheelScrollLines::Auto, true);
        a.next(1, 0.0);
        a.next(1, 50.0);
        a.set_lines(WheelScrollLines::Auto);
        // New gesture: single line even at high velocity.
        assert_eq!(a.next(1, 100.0), 1.0);
    }

    #[test]
    fn default_acceleration_follows_platform() {
        // Local macOS: the OS already accelerates → accelerator is disabled.
        let mac = WheelScrollAccelerator::new(
            WheelScrollLines::Auto,
            None,
            &WheelScrollEnv { is_darwin: true, ssh_free: true },
        );
        assert!(!mac.accelerate);
        // SSH session from macOS: unknown client platform → enabled.
        let ssh = WheelScrollAccelerator::new(
            WheelScrollLines::Auto,
            None,
            &WheelScrollEnv { is_darwin: true, ssh_free: false },
        );
        assert!(ssh.accelerate);
        // Linux local: one event per notch → enabled.
        let linux = WheelScrollAccelerator::new(
            WheelScrollLines::Auto,
            None,
            &WheelScrollEnv { is_darwin: false, ssh_free: true },
        );
        assert!(linux.accelerate);
    }
}
