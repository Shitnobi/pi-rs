//! Port of `packages/tui/src/terminal-image.ts` — T5 slice.
//!
//! T5 covers: capability detection, the capability/override caches, cell
//! dimensions, image-line detection, image-id allocation, the Kitty delete
//! sequences, the Kitty image metadata/placement registry with
//! `cropKittyImageLine`, and (T6) the encoders, cell sizing, image
//! dimension parsers, `renderImage`, `hyperlink` and `imageFallback`.
//!
//! ## Divergences from TS (documented in T5_PLAN)
//!
//! - `allocateImageId` uses `Math.random()`; the Rust default is a
//!   wall-clock-seeded xorshift with an injectable source for tests
//!   ([`set_image_id_random_source`]).
//! - Environment reads go through an injectable lookup for tests; the
//!   default reads `std::env`. `probeTmuxHyperlinks` spawns `tmux` exactly
//!   like the TS `execSync`, but `std::process` has no timeout — tmux exits
//!   promptly on its own.
//! - `process.platform === "win32"` becomes `cfg!(windows)` (compile-time).

use std::cell::{Cell, RefCell};
use std::time::{SystemTime, UNIX_EPOCH};

// =============================================================================
// Types (terminal-image.ts:10-27)
// =============================================================================

/// `ImageProtocol` (terminal-image.ts:10) — `null` is `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProtocol {
    Kitty,
    Iterm2,
}

/// `TerminalCapabilities` (terminal-image.ts:12-16).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCapabilities {
    pub images: Option<ImageProtocol>,
    pub true_color: bool,
    pub hyperlinks: bool,
}

/// `CellDimensions` (terminal-image.ts:18-21).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellDimensions {
    pub width_px: u32,
    pub height_px: u32,
}

// =============================================================================
// Capability detection (terminal-image.ts:44-177)
// =============================================================================

/// Environment lookup: `process.env[name]`. `None` = unset (or invalid
/// UTF-8, which TS cannot represent anyway).
pub type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<String>;

/// The real `process.env` (test divergence: tests inject their own).
fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// `probeTmuxHyperlinks()` (terminal-image.ts:32-50). Runs the same tmux
/// command as the TS `execSync`; on any error falls back to `false`.
fn probe_tmux_hyperlinks() -> bool {
    let output = std::process::Command::new("tmux")
        .args(["display-message", "-p", "#{client_termfeatures}"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    match output {
        Ok(output) => {
            let text = String::from_utf8_lossy(&output.stdout);
            text.split(',').any(|feature| feature.trim() == "hyperlinks")
        }
        Err(_) => false,
    }
}

/// `detectCapabilitiesFromEnvironment` (terminal-image.ts:52-140).
fn detect_capabilities_from_environment(
    env: EnvLookup<'_>,
    is_windows: bool,
    tmux_forwards_hyperlink: &dyn Fn() -> bool,
) -> TerminalCapabilities {
    let term_program = env("TERM_PROGRAM").map(|v| v.to_lowercase()).unwrap_or_default();
    let terminal_emulator =
        env("TERMINAL_EMULATOR").map(|v| v.to_lowercase()).unwrap_or_default();
    let term = env("TERM").map(|v| v.to_lowercase()).unwrap_or_default();
    let color_term = env("COLORTERM").map(|v| v.to_lowercase()).unwrap_or_default();
    let has_true_color_hint =
        color_term == "truecolor" || color_term == "24bit" || term.ends_with("-direct");

    // Emit OSC 8 hyperlinks only when tmux confirms it forwards. Image
    // protocols are unreliable under tmux, so `images: null`.
    if env("TMUX").is_some() || term.starts_with("tmux") {
        return TerminalCapabilities {
            images: None,
            true_color: has_true_color_hint,
            hyperlinks: tmux_forwards_hyperlink(),
        };
    }

    // screen does not forward OSC 8 hyperlinks.
    if term.starts_with("screen") {
        return TerminalCapabilities {
            images: None,
            true_color: has_true_color_hint,
            hyperlinks: false,
        };
    }

    // Herdr forwards OSC 8 and supports Kitty graphics. Check before
    // outer-terminal detection (KITTY_WINDOW_ID can leak into its panes).
    if term_program == "herdr" {
        return TerminalCapabilities {
            images: Some(ImageProtocol::Kitty),
            true_color: has_true_color_hint,
            hyperlinks: true,
        };
    }

    if env("KITTY_WINDOW_ID").is_some() || term_program == "kitty" {
        return TerminalCapabilities {
            images: Some(ImageProtocol::Kitty),
            true_color: true,
            hyperlinks: true,
        };
    }

    if term_program == "ghostty"
        || term.contains("ghostty")
        || env("GHOSTTY_RESOURCES_DIR").is_some()
    {
        return TerminalCapabilities {
            images: Some(ImageProtocol::Kitty),
            true_color: true,
            hyperlinks: true,
        };
    }

    if env("WEZTERM_PANE").is_some() || term_program == "wezterm" {
        return TerminalCapabilities {
            images: Some(ImageProtocol::Kitty),
            true_color: true,
            hyperlinks: true,
        };
    }

    if term_program == "warpterminal"
        || env("WARP_SESSION_ID").is_some()
        || env("WARP_TERMINAL_SESSION_UUID").is_some()
    {
        return TerminalCapabilities {
            images: Some(ImageProtocol::Kitty),
            true_color: true,
            hyperlinks: true,
        };
    }

    if env("ITERM_SESSION_ID").is_some() || term_program == "iterm.app" {
        return TerminalCapabilities {
            images: Some(ImageProtocol::Iterm2),
            true_color: true,
            hyperlinks: true,
        };
    }

    if env("WT_SESSION").is_some() {
        return TerminalCapabilities { images: None, true_color: true, hyperlinks: true };
    }

    if term_program == "alacritty" || term_program == "vscode" || term_program == "zed" {
        return TerminalCapabilities { images: None, true_color: true, hyperlinks: true };
    }

    if terminal_emulator == "jetbrains-jediterm" {
        return TerminalCapabilities { images: None, true_color: true, hyperlinks: false };
    }

    // Windows Terminal does not always set WT_SESSION. Modern Windows
    // consoles support truecolor; keep hyperlinks off unless detected above.
    if is_windows {
        return TerminalCapabilities { images: None, true_color: true, hyperlinks: false };
    }

    // Unknown terminal: be conservative — OSC 8 renders invisibly as "just
    // text" on terminals that swallow it.
    TerminalCapabilities { images: None, true_color: has_true_color_hint, hyperlinks: false }
}

/// `parseBooleanCapabilityOverride` (terminal-image.ts:142-144).
fn parse_boolean_capability_override(value: Option<&str>) -> Option<bool> {
    match value {
        Some("1") => Some(true),
        Some("0") => Some(false),
        _ => None,
    }
}

/// `detectCapabilities` (terminal-image.ts:146-167) with an injected
/// environment (the production entry point is [`detect_capabilities`]).
pub fn detect_capabilities_with(
    env: EnvLookup<'_>,
    is_windows: bool,
    tmux_forwards_hyperlink: &dyn Fn() -> bool,
) -> TerminalCapabilities {
    let hyperlinks = parse_boolean_capability_override(env("PI_HYPERLINKS").as_deref());
    let mut detected = match hyperlinks {
        Some(value) => detect_capabilities_from_environment(env, is_windows, &move || value),
        None => detect_capabilities_from_environment(env, is_windows, tmux_forwards_hyperlink),
    };
    let image_protocol = env("PI_IMAGE_PROTOCOL").map(|v| v.to_lowercase());
    match image_protocol.as_deref() {
        Some("kitty") => detected.images = Some(ImageProtocol::Kitty),
        Some("iterm2") => detected.images = Some(ImageProtocol::Iterm2),
        Some("none") | Some("0") => detected.images = None,
        _ => {}
    }
    if let Some(true_color) = parse_boolean_capability_override(env("PI_TRUE_COLOR").as_deref()) {
        detected.true_color = true_color;
    }
    if let Some(hyperlinks) = hyperlinks {
        detected.hyperlinks = hyperlinks;
    }
    detected
}

/// `detectCapabilities()` (terminal-image.ts:146) — production entry point
/// reading `std::env`.
pub fn detect_capabilities() -> TerminalCapabilities {
    detect_capabilities_with(&process_env, cfg!(windows), &probe_tmux_hyperlinks)
}

// =============================================================================
// Capability cache + overrides (terminal-image.ts:169-204)
// =============================================================================

thread_local! {
    static CACHED_CAPABILITIES: RefCell<Option<TerminalCapabilities>> = const { RefCell::new(None) };
}

/// `Partial<TerminalCapabilities>` — `None` = not overridden (the nested
/// `Option` in `images` expresses "overridden to `null`").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CapabilityOverrides {
    pub images: Option<Option<ImageProtocol>>,
    pub true_color: Option<bool>,
    pub hyperlinks: Option<bool>,
}

thread_local! {
    static CAPABILITY_OVERRIDES: Cell<CapabilityOverrides> = const {
        Cell::new(CapabilityOverrides { images: None, true_color: None, hyperlinks: None })
    };
}

/// `getCapabilities()` (terminal-image.ts:169-177). Module-level cache →
/// thread-local (cargo test parallelism).
pub fn get_capabilities() -> TerminalCapabilities {
    CACHED_CAPABILITIES.with(|cached| {
        let mut cached = cached.borrow_mut();
        if cached.is_none() {
            let overrides = CAPABILITY_OVERRIDES.with(|o| o.get());
            let mut caps = detect_capabilities();
            if let Some(images) = overrides.images {
                caps.images = images;
            }
            if let Some(true_color) = overrides.true_color {
                caps.true_color = true_color;
            }
            if let Some(hyperlinks) = overrides.hyperlinks {
                caps.hyperlinks = hyperlinks;
            }
            *cached = Some(caps);
        }
        cached.expect("just populated")
    })
}

/// `resetCapabilitiesCache()` (terminal-image.ts:183).
pub fn reset_capabilities_cache() {
    CACHED_CAPABILITIES.with(|c| *c.borrow_mut() = None);
}

/// `setCapabilityOverrides` (terminal-image.ts:189-201).
pub fn set_capability_overrides(overrides: CapabilityOverrides) {
    CAPABILITY_OVERRIDES.with(|o| {
        if o.get() == overrides {
            return;
        }
        o.set(overrides);
    });
    reset_capabilities_cache();
}

/// `setCapabilities` (terminal-image.ts:203) — tests exercise both paths.
pub fn set_capabilities(caps: TerminalCapabilities) {
    CACHED_CAPABILITIES.with(|c| *c.borrow_mut() = Some(caps));
}

// =============================================================================
// Cell dimensions (terminal-image.ts:29-42)
// =============================================================================

thread_local! {
    static CELL_DIMENSIONS: Cell<CellDimensions> =
        const { Cell::new(CellDimensions { width_px: 9, height_px: 18 }) };
}

/// `getCellDimensions()` (terminal-image.ts:39).
pub fn get_cell_dimensions() -> CellDimensions {
    CELL_DIMENSIONS.with(|c| c.get())
}

/// `setCellDimensions()` (terminal-image.ts:42) — updated by the TUI when
/// the terminal answers the cell-size query.
pub fn set_cell_dimensions(width_px: u32, height_px: u32) {
    CELL_DIMENSIONS.with(|c| c.set(CellDimensions { width_px, height_px }));
}

// =============================================================================
// Image lines + IDs + delete sequences (terminal-image.ts:206-319)
// =============================================================================

/// `KITTY_PREFIX` (terminal-image.ts:206).
pub const KITTY_PREFIX: &str = "\x1b_G";
/// `ITERM2_PREFIX` (terminal-image.ts:207).
pub const ITERM2_PREFIX: &str = "\x1b]1337;File=";

/// `isImageLine` (terminal-image.ts:209-214). Promoted from `component.rs`
/// where the T4 slice carried it inline.
pub fn is_image_line(line: &str) -> bool {
    // Fast path: sequence at line start (single-row images); slow path:
    // sequence elsewhere (multi-row images have cursor-up prefix).
    line.starts_with(KITTY_PREFIX)
        || line.starts_with(ITERM2_PREFIX)
        || line.contains(KITTY_PREFIX)
        || line.contains(ITERM2_PREFIX)
}

thread_local! {
    /// Injectable `Math.random` replacement (T5_PLAN divergence).
    static IMAGE_ID_RANDOM: RefCell<Option<Box<dyn FnMut() -> f64>>> = const { RefCell::new(None) };
    /// xorshift64 state for the default random source.
    static IMAGE_ID_STATE: Cell<u64> = const { Cell::new(0) };
}

/// Replace the `Math.random()` source used by [`allocate_image_id`]
/// (`None` restores the default). Test hook — the TS has no equivalent.
pub fn set_image_id_random_source(source: Option<Box<dyn FnMut() -> f64>>) {
    IMAGE_ID_RANDOM.with(|s| *s.borrow_mut() = source);
    IMAGE_ID_STATE.with(|s| s.set(0));
}

fn seed_from_time() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E3779B97F4A7C15);
    // xorshift-mix the timestamp: splitmix64 finalizer.
    let mut z = nanos.wrapping_add(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// `allocateImageId()` (terminal-image.ts:216-222) — random ID in
/// `[1, 0xffffffff]` to avoid collisions between module instances.
/// `Math.random` → wall-clock-seeded xorshift (see module docs).
pub fn allocate_image_id() -> i64 {
    let r = IMAGE_ID_RANDOM.with(|source| {
        let mut source = source.borrow_mut();
        match source.as_mut() {
            Some(random) => random(),
            None => {
                let mut x = IMAGE_ID_STATE.with(|state| {
                    let mut x = state.get();
                    if x == 0 {
                        x = seed_from_time();
                    }
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    state.set(x);
                    x
                });
                // Re-mix so consecutive IDs in the same nanosecond differ.
                x = x.wrapping_mul(0x2545F4914F6CDD1D);
                (x >> 11) as f64 / (1u64 << 53) as f64
            }
        }
    });
    (r * 0xFFFF_FFFEu64 as f64).floor() as i64 + 1
}

/// `deleteKittyImage` (terminal-image.ts:287-290) — uppercase 'I' also
/// frees the image data.
pub fn delete_kitty_image(image_id: i64) -> String {
    format!("\x1b_Ga=d,d=I,i={image_id},q=2\x1b\\")
}

/// `deleteAllKittyImages` (terminal-image.ts:295-297) — uppercase 'A' also
/// frees the image data.
pub fn delete_all_kitty_images() -> &'static str {
    "\x1b_Ga=d,d=A,q=2\x1b\\"
}

/// `deleteAllKittyPlacements` (terminal-image.ts:300) — keeps the uploaded
/// image data.
pub fn delete_all_kitty_placements() -> &'static str {
    "\x1b_Ga=d,d=a,q=2\x1b\\"
}

// =============================================================================
// Kitty image metadata + placement registry (terminal-image.ts:326-478)
// =============================================================================

/// `KittyImageMetadata` (terminal-image.ts:326-334).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KittyImageMetadata {
    pub image_id: i64,
    pub columns: i64,
    pub rows: i64,
    pub width_px: i64,
    pub height_px: i64,
}

/// Internal registry entry (terminal-image.ts:336-346). Insertion order is
/// preserved with a `Vec`; the >1000 cap evicts the oldest entry.
struct RegisteredKittyImageMetadata {
    metadata: KittyImageMetadata,
    transmission_generation: i64,
}

thread_local! {
    static KITTY_IMAGE_METADATA: RefCell<Vec<RegisteredKittyImageMetadata>> = const { RefCell::new(Vec::new()) };
    static KITTY_TRANSMISSION_GENERATION: Cell<i64> = const { Cell::new(0) };
}

/// `registerKittyImageMetadata` (terminal-image.ts:349-359).
pub fn register_kitty_image_metadata(metadata: KittyImageMetadata) {
    let transmission_generation = KITTY_TRANSMISSION_GENERATION.with(|generation| {
        generation.set(generation.get() + 1);
        generation.get()
    });
    KITTY_IMAGE_METADATA.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.retain(|entry| entry.metadata.image_id != metadata.image_id);
        registry.push(RegisteredKittyImageMetadata { metadata, transmission_generation });
        if registry.len() > 1000 {
            registry.remove(0);
        }
    });
}

/// `/\x1b_G([^;]*);/` — the first Kitty control block in the line, as
/// `(byte start, byte match length, controls)` (terminal-image.ts:380-384).
fn find_kitty_controls(line: &str) -> Option<(usize, usize, &str)> {
    let start = line.find(KITTY_PREFIX)?;
    let controls_start = start + KITTY_PREFIX.len();
    let semi = controls_start + line[controls_start..].find(';')?;
    Some((start, semi + 1 - start, &line[controls_start..semi]))
}

/// `String.prototype.indexOf(needle, from)`.
fn find_from(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    if from > haystack.len() {
        return None;
    }
    haystack[from..].find(needle).map(|i| i + from)
}

/// `/(?:^|,)KEY=(\d+)(?:,|$)/` over comma-separated controls.
fn control_digits(controls: &str, key: &str) -> Option<i64> {
    let prefix = format!("{key}=");
    for part in controls.split(',') {
        if let Some(value) = part.strip_prefix(&prefix) {
            if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
                return value.parse().ok();
            }
        }
    }
    None
}

/// `/(?:^|,)KEY(?:,|$)/` — an exact comma-separated part.
fn has_flag_control(controls: &str, key: &str) -> bool {
    controls.split(',').any(|part| part == key)
}

/// `getRegisteredKittyImageMetadataFromControls` (terminal-image.ts:361-363):
/// `/(?:^|,)i=(\d+)(?:,|$)/` over the controls.
fn registered_metadata_from_controls(controls: &str) -> Option<RegisteredKittyImageMetadata> {
    let image_id = control_digits(controls, "i")?;
    KITTY_IMAGE_METADATA.with(|registry| {
        registry
            .borrow()
            .iter()
            .find(|entry| entry.metadata.image_id == image_id)
            .map(|entry| RegisteredKittyImageMetadata {
                metadata: entry.metadata,
                transmission_generation: entry.transmission_generation,
            })
    })
}

/// `getKittyImageMetadata` (terminal-image.ts:380-388).
pub fn get_kitty_image_metadata(line: &str) -> Option<KittyImageMetadata> {
    let (_, _, controls) = find_kitty_controls(line)?;
    registered_metadata_from_controls(controls).map(|entry| entry.metadata)
}

/// `getExplicitKittyImageRows` (terminal-image.ts:374-378).
fn get_explicit_kitty_image_rows(controls: &str) -> Option<i64> {
    let rows = control_digits(controls, "r")?;
    if rows > 0 {
        Some(rows)
    } else {
        None
    }
}

/// `getKittyImageRowsFromControls` (terminal-image.ts:379).
fn get_kitty_image_rows_from_controls(controls: &str, fallback_rows: i64) -> i64 {
    get_explicit_kitty_image_rows(controls).unwrap_or(fallback_rows)
}

/// `KITTY_PLACEMENT_CONTROL_KEYS` (terminal-image.ts:390-408).
const KITTY_PLACEMENT_CONTROL_KEYS: [&str; 17] =
    ["i", "p", "x", "y", "w", "h", "X", "Y", "c", "r", "C", "U", "z", "P", "Q", "H", "V"];

/// `getKittyImagePlacementRows` (terminal-image.ts:410-416) — rows covered
/// by an image placement without scanning its payload.
pub fn get_kitty_image_placement_rows(line: &str) -> Option<i64> {
    let (_, _, controls) = find_kitty_controls(line)?;
    if let Some(explicit_rows) = get_explicit_kitty_image_rows(controls) {
        return Some(explicit_rows);
    }
    registered_metadata_from_controls(controls).map(|entry| entry.metadata.rows)
}

/// `KittyImagePlacement` (terminal-image.ts:348-356).
#[derive(Debug, Clone, PartialEq)]
pub struct KittyImagePlacement {
    pub image_id: i64,
    pub transmission_generation: i64,
    pub transmission_bytes: i64,
    pub estimated_decoded_bytes: i64,
    pub rows: i64,
    pub sequence: String,
    pub replacement_line: String,
}

/// `getKittyImagePlacement` (terminal-image.ts:418-456) — build a
/// placement-only command for an image line emitted by `renderImage`.
pub fn get_kitty_image_placement(line: &str) -> Option<KittyImagePlacement> {
    let (match_start, _, controls) = find_kitty_controls(line)?;
    let metadata = registered_metadata_from_controls(controls)?;

    let mut command_start = match_start;
    let mut command_controls = controls;
    let mut transmission_end;
    loop {
        let terminator = find_from(line, "\x1b\\", command_start + KITTY_PREFIX.len())?;
        transmission_end = terminator + "\x1b\\".len();
        // Continue chunked (`m=1`) transmissions.
        if !has_flag_control(command_controls, "m=1") {
            break;
        }
        command_start = transmission_end;
        if !line[command_start..].starts_with(KITTY_PREFIX) {
            return None;
        }
        let controls_end = find_from(line, ";", command_start + KITTY_PREFIX.len())?;
        command_controls = &line[command_start + KITTY_PREFIX.len()..controls_end];
    }

    let filtered: Vec<&str> = controls
        .split(',')
        .filter(|control| {
            let key = control.split('=').next().unwrap_or("");
            KITTY_PLACEMENT_CONTROL_KEYS.contains(&key)
        })
        .collect();
    let sequence = format!("\x1b_Ga=p,q=2,{}\x1b\\", filtered.join(","));
    Some(KittyImagePlacement {
        image_id: metadata.metadata.image_id,
        transmission_generation: metadata.transmission_generation,
        transmission_bytes: (transmission_end - match_start) as i64,
        estimated_decoded_bytes: metadata.metadata.width_px * metadata.metadata.height_px * 4,
        rows: get_kitty_image_rows_from_controls(controls, metadata.metadata.rows),
        replacement_line: format!(
            "{}{}{}",
            &line[..match_start],
            sequence,
            &line[transmission_end..]
        ),
        sequence,
    })
}

/// `cropKittyImageLine` (terminal-image.ts:458-478).
pub fn crop_kitty_image_line(line: &str, hidden_rows: i64, visible_rows: i64) -> String {
    let metadata = match get_kitty_image_metadata(line) {
        Some(metadata) => metadata,
        None => return line.to_string(),
    };
    let (start, match_len, controls) = match find_kitty_controls(line) {
        Some(found) => found,
        None => return line.to_string(),
    };
    if hidden_rows < 0 || hidden_rows >= metadata.rows || visible_rows <= 0 {
        return line.to_string();
    }
    let cropped_rows = visible_rows.min(metadata.rows - hidden_rows);
    if hidden_rows == 0 && cropped_rows == metadata.rows {
        return line.to_string();
    }
    let source_y = ((metadata.height_px as f64 * hidden_rows as f64) / metadata.rows as f64).floor();
    let source_end =
        ((metadata.height_px as f64 * (hidden_rows + cropped_rows) as f64) / metadata.rows as f64)
            .ceil();
    let source_height = (1.0f64).max((metadata.height_px as f64).min(source_end) - source_y);
    // `controls.filter((control) => !/^[yhr]=/.test(control))` then push the
    // cropped geometry — an empty filter result joins without a leading
    // comma.
    let mut rebuilt = String::new();
    let filtered: Vec<&str> = controls
        .split(',')
        .filter(|control| {
            !control.starts_with("y=") && !control.starts_with("h=") && !control.starts_with("r=")
        })
        .collect();
    for (index, control) in filtered.iter().enumerate() {
        if index > 0 {
            rebuilt.push(',');
        }
        rebuilt.push_str(control);
    }
    if !filtered.is_empty() {
        rebuilt.push(',');
    }
    rebuilt.push_str(&format!(
        "y={},h={},r={}",
        source_y as i64,
        source_height as i64,
        cropped_rows
    ));
    format!("{}\x1b_G{};{}", &line[..start], rebuilt, &line[start + match_len..])
}

// =============================================================================
// =============================================================================
// Encoders (terminal-image.ts:226-348) — T6
// =============================================================================

/// `ImageDimensions` (terminal-image.ts:24-27).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageDimensions {
    pub width_px: i64,
    pub height_px: i64,
}

/// `ImageCellSize` (terminal-image.ts:318-321).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageCellSize {
    pub columns: i64,
    pub rows: i64,
}

/// `encodeKitty` options (terminal-image.ts:226-235). `None` mirrors
/// `undefined`; TS truthiness (`if (options.columns)`) means `Some(0)` is
/// treated as absent.
#[derive(Debug, Clone, Copy, Default)]
pub struct KittyEncodeOptions {
    pub columns: Option<i64>,
    pub rows: Option<i64>,
    /// Kitty image ID. If set, reuses/replaces the existing image.
    pub image_id: Option<i64>,
    /// Whether Kitty should apply its default cursor movement (default true).
    pub move_cursor: Option<bool>,
}

/// `encodeKitty` (terminal-image.ts:226-274) — chunked transmission for
/// payloads over 4096 chars.
pub fn encode_kitty(base64_data: &str, options: &KittyEncodeOptions) -> String {
    const CHUNK_SIZE: usize = 4096;

    let mut params: Vec<String> = vec!["a=T".into(), "f=100".into(), "q=2".into()];
    if options.move_cursor == Some(false) {
        params.push("C=1".into());
    }
    // TS truthiness: `if (options.columns)` skips `undefined` and `0`
    // (negatives are truthy in JS).
    if let Some(columns) = options.columns {
        if columns != 0 {
            params.push(format!("c={columns}"));
        }
    }
    if let Some(rows) = options.rows {
        if rows != 0 {
            params.push(format!("r={rows}"));
        }
    }
    if let Some(image_id) = options.image_id {
        if image_id != 0 {
            params.push(format!("i={image_id}"));
        }
    }

    if base64_data.len() <= CHUNK_SIZE {
        return format!("\x1b_G{};{}\x1b\\", params.join(","), base64_data);
    }

    let mut chunks: Vec<String> = Vec::new();
    let mut offset = 0usize;
    let mut is_first = true;
    while offset < base64_data.len() {
        let chunk = &base64_data[offset..(offset + CHUNK_SIZE).min(base64_data.len())];
        let is_last = offset + CHUNK_SIZE >= base64_data.len();
        if is_first {
            chunks.push(format!("\x1b_G{},m=1;{}\x1b\\", params.join(","), chunk));
            is_first = false;
        } else if is_last {
            chunks.push(format!("\x1b_Gm=0;{}\x1b\\", chunk));
        } else {
            chunks.push(format!("\x1b_Gm=1;{}\x1b\\", chunk));
        }
        offset += CHUNK_SIZE;
    }
    chunks.join("")
}

/// `encodeITerm2` options (terminal-image.ts:293-302). `width`/`height` are
/// number-or-string in TS; the port stores the already-stringified form
/// (`"2"`, `"auto"`).
#[derive(Debug, Clone, Default)]
pub struct Iterm2EncodeOptions {
    pub width: Option<String>,
    pub height: Option<String>,
    pub name: Option<String>,
    pub preserve_aspect_ratio: Option<bool>,
    pub inline: Option<bool>,
}

/// `encodeITerm2` (terminal-image.ts:293-327).
pub fn encode_iterm2(base64_data: &str, options: &Iterm2EncodeOptions) -> String {
    let inline = options.inline.unwrap_or(true);
    let mut params: Vec<String> = vec![
        format!("inline={}", if inline { 1 } else { 0 }),
        format!("size={}", base64_byte_length(base64_data)),
    ];
    if let Some(width) = &options.width {
        params.push(format!("width={width}"));
    }
    if let Some(height) = &options.height {
        params.push(format!("height={height}"));
    }
    if let Some(name) = &options.name {
        if !name.is_empty() {
            params.push(format!("name={}", base64_encode(name.as_bytes())));
        }
    }
    if options.preserve_aspect_ratio == Some(false) {
        params.push("preserveAspectRatio=0".into());
    }
    format!("\x1b]1337;File={}:{}\x07", params.join(";"), base64_data)
}

// =============================================================================
// Base64 helpers (Node `Buffer` semantics, lenient decode)
// =============================================================================

/// Decode one base64 char to its 6-bit value. Node's `Buffer.from(s,
/// "base64")` accepts both the standard and URL-safe alphabets and ignores
/// every other character.
fn base64_char_value(c: u8) -> Option<u32> {
    match c {
        b'A'..=b'Z' => Some((c - b'A') as u32),
        b'a'..=b'z' => Some((c - b'a') as u32 + 26),
        b'0'..=b'9' => Some((c - b'0') as u32 + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    }
}

/// `Buffer.from(s, "base64")` — lenient decode: non-alphabet characters
/// (including `=` and whitespace) are skipped.
pub fn base64_decode(input: &str) -> Vec<u8> {
    let values: Vec<u32> = input.bytes().filter_map(base64_char_value).collect();
    let mut out = Vec::with_capacity(values.len() / 4 * 3 + 3);
    for group in values.chunks(4) {
        match group.len() {
            4 => {
                out.push(((group[0] << 2) | (group[1] >> 4)) as u8);
                out.push((((group[1] & 0x0F) << 4) | (group[2] >> 2)) as u8);
                out.push((((group[2] & 0x03) << 6) | group[3]) as u8);
            }
            3 => {
                out.push(((group[0] << 2) | (group[1] >> 4)) as u8);
                out.push((((group[1] & 0x0F) << 4) | (group[2] >> 2)) as u8);
            }
            2 => {
                out.push(((group[0] << 2) | (group[1] >> 4)) as u8);
            }
            _ => {} // 1 leftover char decodes to nothing
        }
    }
    out
}

/// `Buffer.byteLength(s, "base64")` — the decoded byte count.
pub fn base64_byte_length(input: &str) -> usize {
    base64_decode(input).len()
}

/// `Buffer.from(s).toString("base64")` — strict standard encoding with
/// padding.
pub fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for group in data.chunks(3) {
        let b = [group[0], group.get(1).copied().unwrap_or(0), group.get(2).copied().unwrap_or(0)];
        out.push(ALPHABET[(b[0] >> 2) as usize] as char);
        out.push(ALPHABET[(((b[0] & 0x03) << 4) | (b[1] >> 4)) as usize] as char);
        if group.len() > 1 {
            out.push(ALPHABET[(((b[1] & 0x0F) << 2) | (b[2] >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if group.len() > 2 {
            out.push(ALPHABET[(b[2] & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

// =============================================================================
// Image cell sizing (terminal-image.ts:472-527)
// =============================================================================

/// `chooseLessDistortedCellCount` (terminal-image.ts:472-479).
fn choose_less_distorted_cell_count(upper_count: i64, ideal_count: f64) -> i64 {
    if upper_count <= 1 {
        return upper_count;
    }
    let lower_count = upper_count - 1;
    let upper_distortion = (upper_count as f64 / ideal_count).max(ideal_count / upper_count as f64);
    let lower_distortion = (lower_count as f64 / ideal_count).max(ideal_count / lower_count as f64);
    if lower_distortion < upper_distortion {
        lower_count
    } else {
        upper_count
    }
}

/// `calculateImageCellSize` (terminal-image.ts:481-514). Kitty callers pass
/// `optimize_aspect_ratio = true` to trade a cell for less distortion;
/// iTerm2 keeps the plain ceiling sizes.
pub fn calculate_image_cell_size(
    image_dimensions: ImageDimensions,
    max_width_cells: i64,
    max_height_cells: Option<i64>,
    cell_dimensions: CellDimensions,
    optimize_aspect_ratio: bool,
) -> ImageCellSize {
    let max_width = 1i64.max(max_width_cells);
    let max_height = max_height_cells.map(|h| 1i64.max(h));
    let image_width = 1i64.max(image_dimensions.width_px) as f64;
    let image_height = 1i64.max(image_dimensions.height_px) as f64;
    let cell_w = cell_dimensions.width_px as f64;
    let cell_h = cell_dimensions.height_px as f64;

    let width_scale = (max_width as f64 * cell_w) / image_width;
    let height_scale = match max_height {
        None => width_scale,
        Some(max_height) => (max_height as f64 * cell_h) / image_height,
    };
    let scale = width_scale.min(height_scale);

    let scaled_width_px = image_width * scale;
    let scaled_height_px = image_height * scale;
    let mut columns = 1i64.max(max_width.min((scaled_width_px / cell_w).ceil() as i64));
    let height_rows = scaled_height_px / cell_h;
    let mut rows = 1i64.max(height_rows.ceil() as i64);
    if let Some(max_height) = max_height {
        rows = rows.min(max_height);
    }

    if !optimize_aspect_ratio {
        return ImageCellSize { columns, rows };
    }

    if width_scale <= height_scale {
        let ideal_rows = (columns as f64 * cell_w * image_height) / (image_width * cell_h);
        rows = choose_less_distorted_cell_count(rows, ideal_rows);
    } else {
        let ideal_columns = (rows as f64 * cell_h * image_width) / (image_height * cell_w);
        columns = choose_less_distorted_cell_count(columns, ideal_columns);
    }

    ImageCellSize { columns, rows }
}

/// `calculateImageRows` (terminal-image.ts:521-527).
pub fn calculate_image_rows(
    image_dimensions: ImageDimensions,
    target_width_cells: i64,
    cell_dimensions: CellDimensions,
) -> i64 {
    calculate_image_cell_size(image_dimensions, target_width_cells, None, cell_dimensions, false).rows
}

// =============================================================================
// Image dimension parsers (terminal-image.ts:529-668)
// =============================================================================

/// `getPngDimensions` (terminal-image.ts:529-548) — PNG IHDR width/height
/// at byte offsets 16/20 (big-endian).
pub fn get_png_dimensions(base64_data: &str) -> Option<ImageDimensions> {
    let buffer = base64_decode(base64_data);
    if buffer.len() < 24 {
        return None;
    }
    if buffer[0] != 0x89 || buffer[1] != 0x50 || buffer[2] != 0x4E || buffer[3] != 0x47 {
        return None;
    }
    let width = u32::from_be_bytes([buffer[16], buffer[17], buffer[18], buffer[19]]);
    let height = u32::from_be_bytes([buffer[20], buffer[21], buffer[22], buffer[23]]);
    Some(ImageDimensions { width_px: width as i64, height_px: height as i64 })
}

/// `getJpegDimensions` (terminal-image.ts:550-568) — the TS stub always
/// returns null (marker scanning is not implemented upstream).
pub fn get_jpeg_dimensions(_base64_data: &str) -> Option<ImageDimensions> {
    None
}

/// `getGifDimensions` (terminal-image.ts:593-613) — logical screen size at
/// offsets 6/8 (little-endian).
pub fn get_gif_dimensions(base64_data: &str) -> Option<ImageDimensions> {
    let buffer = base64_decode(base64_data);
    if buffer.len() < 10 {
        return None;
    }
    let sig = &buffer[0..6];
    if sig != b"GIF87a" && sig != b"GIF89a" {
        return None;
    }
    let width = u16::from_le_bytes([buffer[6], buffer[7]]);
    let height = u16::from_le_bytes([buffer[8], buffer[9]]);
    Some(ImageDimensions { width_px: width as i64, height_px: height as i64 })
}

/// `getWebpDimensions` (terminal-image.ts:615-652) — VP8 / VP8L / VP8X
/// dimension fields.
pub fn get_webp_dimensions(base64_data: &str) -> Option<ImageDimensions> {
    let buffer = base64_decode(base64_data);
    if buffer.len() < 30 {
        return None;
    }
    if &buffer[0..4] != b"RIFF" || &buffer[8..12] != b"WEBP" {
        return None;
    }
    let chunk = &buffer[12..16];
    if chunk == b"VP8 " {
        let width = u16::from_le_bytes([buffer[26], buffer[27]]) & 0x3FFF;
        let height = u16::from_le_bytes([buffer[28], buffer[29]]) & 0x3FFF;
        Some(ImageDimensions { width_px: width as i64, height_px: height as i64 })
    } else if chunk == b"VP8L" {
        if buffer.len() < 25 {
            return None;
        }
        let bits = u32::from_le_bytes([buffer[21], buffer[22], buffer[23], buffer[24]]);
        let width = (bits & 0x3FFF) + 1;
        let height = ((bits >> 14) & 0x3FFF) + 1;
        Some(ImageDimensions { width_px: width as i64, height_px: height as i64 })
    } else if chunk == b"VP8X" {
        let width = (buffer[24] as u32 | (buffer[25] as u32) << 8 | (buffer[26] as u32) << 16) + 1;
        let height = (buffer[27] as u32 | (buffer[28] as u32) << 8 | (buffer[29] as u32) << 16) + 1;
        Some(ImageDimensions { width_px: width as i64, height_px: height as i64 })
    } else {
        None
    }
}

/// `getImageDimensions` (terminal-image.ts:654-668) — dispatch by MIME type.
pub fn get_image_dimensions(base64_data: &str, mime_type: &str) -> Option<ImageDimensions> {
    match mime_type {
        "image/png" => get_png_dimensions(base64_data),
        "image/jpeg" => get_jpeg_dimensions(base64_data),
        "image/gif" => get_gif_dimensions(base64_data),
        "image/webp" => get_webp_dimensions(base64_data),
        _ => None,
    }
}

// =============================================================================
// renderImage (terminal-image.ts:670-730)
// =============================================================================

/// `ImageRenderOptions` (terminal-image.ts:29-37).
#[derive(Debug, Clone, Copy, Default)]
pub struct ImageRenderOptions {
    pub max_width_cells: Option<i64>,
    pub max_height_cells: Option<i64>,
    pub preserve_aspect_ratio: Option<bool>,
    /// Kitty image ID. If provided, reuses/replaces the existing image.
    pub image_id: Option<i64>,
    /// Whether Kitty should apply its default cursor movement (default true).
    pub move_cursor: Option<bool>,
}

/// `renderImage`'s result (terminal-image.ts:670-679).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedImage {
    pub sequence: String,
    pub columns: i64,
    pub rows: i64,
    pub image_id: Option<i64>,
}

/// `renderImage` (terminal-image.ts:670-730) — `None` when the terminal has
/// no image protocol.
pub fn render_image(
    base64_data: &str,
    image_dimensions: ImageDimensions,
    options: &ImageRenderOptions,
) -> Option<RenderedImage> {
    let caps = get_capabilities();
    let protocol = caps.images?;

    let max_width = options.max_width_cells.unwrap_or(80);
    // Reduce Kitty's cell-aligned distortion without shrinking iTerm2
    // reservations.
    let size = calculate_image_cell_size(
        image_dimensions,
        max_width,
        options.max_height_cells,
        get_cell_dimensions(),
        protocol == ImageProtocol::Kitty,
    );

    match protocol {
        ImageProtocol::Kitty => {
            // TS: `options.imageId !== undefined` (Some(0) still registers).
            if let Some(image_id) = options.image_id {
                register_kitty_image_metadata(KittyImageMetadata {
                    image_id,
                    columns: size.columns,
                    rows: size.rows,
                    width_px: image_dimensions.width_px,
                    height_px: image_dimensions.height_px,
                });
            }
            let sequence = encode_kitty(
                base64_data,
                &KittyEncodeOptions {
                    columns: Some(size.columns),
                    rows: Some(size.rows),
                    image_id: options.image_id,
                    move_cursor: options.move_cursor,
                },
            );
            Some(RenderedImage {
                sequence,
                columns: size.columns,
                rows: size.rows,
                image_id: options.image_id,
            })
        }
        ImageProtocol::Iterm2 => {
            let sequence = encode_iterm2(
                base64_data,
                &Iterm2EncodeOptions {
                    width: Some(size.columns.to_string()),
                    height: Some("auto".to_string()),
                    name: None,
                    preserve_aspect_ratio: Some(options.preserve_aspect_ratio.unwrap_or(true)),
                    inline: None,
                },
            );
            Some(RenderedImage { sequence, columns: size.columns, rows: size.rows, image_id: None })
        }
    }
}

// =============================================================================
// hyperlink / imageFallback (terminal-image.ts:732-762)
// =============================================================================

/// `hyperlink` (terminal-image.ts:732-746) — OSC 8.
pub fn hyperlink(text: &str, url: &str) -> String {
    format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
}

/// `os.homedir()` (documented divergence: env-only, no passwd fallback —
/// tests always run with `HOME` set).
fn home_dir() -> Option<String> {
    if cfg!(windows) {
        std::env::var("USERPROFILE").ok().filter(|s| !s.is_empty())
    } else {
        std::env::var("HOME").ok().filter(|s| !s.is_empty())
    }
}

/// `shortenImagePath` (terminal-image.ts:737-748) — `~/...` for
/// home-prefixed absolute paths.
fn shorten_image_path(filename: &str) -> String {
    let home = home_dir().unwrap_or_default();
    if !home.is_empty()
        && (filename == home
            || filename.starts_with(&format!("{home}/"))
            || filename.starts_with(&format!("{home}\\")))
    {
        return format!("~{}", &filename[home.len()..]);
    }
    filename.to_string()
}

/// `url.pathToFileURL(path).href` for absolute paths (WHATWG file URL):
/// percent-encodes the file path percent-encode set (C0 controls, DEL,
/// space, `"#<>?\`{}` and non-ASCII bytes).
fn file_url_from_path(filename: &str) -> String {
    #[cfg(windows)]
    {
        // `file:///C:/path/with/forward/slashes`.
        let normalized = filename.replace('\\', "/");
        let mut url = String::from("file:///");
        for b in normalized.bytes() {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/' | b':') {
                url.push(b as char);
            } else {
                url.push_str(&format!("%{b:02X}"));
            }
        }
        return url;
    }
    #[cfg(not(windows))]
    {
        let mut url = String::from("file://");
        for b in filename.bytes() {
            let encode = b < 0x21
                || b > 0x7E
                || matches!(b, b'"' | b'#' | b'<' | b'>' | b'?' | b'`' | b'{' | b'}');
            if encode {
                url.push_str(&format!("%{b:02X}"));
            } else {
                url.push(b as char);
            }
        }
        url
    }
}

/// `imageFallback` (terminal-image.ts:750-762) — text fallback when the
/// terminal cannot render inline images.
pub fn image_fallback(
    mime_type: &str,
    dimensions: Option<ImageDimensions>,
    filename: Option<&str>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(filename) = filename {
        let display = shorten_image_path(filename);
        if get_capabilities().hyperlinks && std::path::Path::new(filename).is_absolute() {
            parts.push(hyperlink(&display, &file_url_from_path(filename)));
        } else {
            parts.push(display);
        }
    }
    parts.push(format!("[{mime_type}]"));
    if let Some(dimensions) = dimensions {
        parts.push(format!("{}x{}", dimensions.width_px, dimensions.height_px));
    }
    format!("[Image: {}]", parts.join(" "))
}

// =============================================================================
// Test support
// =============================================================================
// =============================================================================

/// Reset all module state (registries, caches, random source). The TS tests
/// recreate the module per file; Rust tests share the thread-local state.
pub fn reset_for_test() {
    KITTY_IMAGE_METADATA.with(|registry| registry.borrow_mut().clear());
    KITTY_TRANSMISSION_GENERATION.with(|generation| generation.set(0));
    reset_capabilities_cache();
    CAPABILITY_OVERRIDES.with(|overrides| overrides.set(CapabilityOverrides::default()));
    CELL_DIMENSIONS.with(|dims| dims.set(CellDimensions { width_px: 9, height_px: 18 }));
    set_image_id_random_source(None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::global_test_lock;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn image_line_detection() {
        assert!(is_image_line("\x1b_Ga=T,f=100;q=2\x1b\\"));
        assert!(is_image_line("\x1b]1337;File=inline=1:x\x07"));
        // Multi-row images carry a cursor-up prefix.
        assert!(is_image_line("\x1b[2A\x1b_Ga=T;q=2\x1b\\"));
        assert!(!is_image_line("plain text"));
        assert!(!is_image_line("\x1b[31mred\x1b[0m"));
    }

    #[test]
    fn detect_kitty_via_window_id() {
        let caps = detect_capabilities_with(&env(&[("KITTY_WINDOW_ID", "1")]), false, &|| false);
        assert_eq!(
            caps,
            TerminalCapabilities {
                images: Some(ImageProtocol::Kitty),
                true_color: true,
                hyperlinks: true
            }
        );
    }

    #[test]
    fn detect_tmux_disables_images() {
        let caps = detect_capabilities_with(&env(&[("TMUX", "/tmp/x")]), false, &|| true);
        assert_eq!(caps.images, None);
        assert!(caps.hyperlinks); // probe says forwarded
        let caps = detect_capabilities_with(&env(&[("TERM", "tmux-256color")]), false, &|| false);
        assert_eq!(caps.images, None);
        assert!(!caps.hyperlinks);
    }

    #[test]
    fn detect_screen_disables_hyperlinks() {
        let caps = detect_capabilities_with(&env(&[("TERM", "screen-256color")]), false, &|| true);
        assert_eq!(caps.images, None);
        assert!(!caps.hyperlinks);
        assert!(!caps.true_color);
    }

    #[test]
    fn detect_wezterm_and_iterm() {
        let caps = detect_capabilities_with(&env(&[("WEZTERM_PANE", "0")]), false, &|| false);
        assert_eq!(caps.images, Some(ImageProtocol::Kitty));
        let caps = detect_capabilities_with(&env(&[("ITERM_SESSION_ID", "w0")]), false, &|| false);
        assert_eq!(caps.images, Some(ImageProtocol::Iterm2));
    }

    #[test]
    fn detect_unknown_terminal_is_conservative() {
        let caps = detect_capabilities_with(&env(&[]), false, &|| false);
        assert_eq!(
            caps,
            TerminalCapabilities { images: None, true_color: false, hyperlinks: false }
        );
        let caps =
            detect_capabilities_with(&env(&[("COLORTERM", "truecolor")]), false, &|| false);
        assert!(caps.true_color);
    }

    #[test]
    fn detect_env_overrides() {
        let caps = detect_capabilities_with(
            &env(&[("KITTY_WINDOW_ID", "1"), ("PI_IMAGE_PROTOCOL", "none")]),
            false,
            &|| false,
        );
        assert_eq!(caps.images, None);
        let caps = detect_capabilities_with(
            &env(&[("TERM", "xterm"), ("PI_IMAGE_PROTOCOL", "iterm2")]),
            false,
            &|| false,
        );
        assert_eq!(caps.images, Some(ImageProtocol::Iterm2));
        let caps = detect_capabilities_with(
            &env(&[("TERM", "xterm"), ("PI_IMAGE_PROTOCOL", "0")]),
            false,
            &|| false,
        );
        assert_eq!(caps.images, None);
        let caps = detect_capabilities_with(
            &env(&[("TERM", "xterm"), ("PI_HYPERLINKS", "1"), ("PI_TRUE_COLOR", "1")]),
            false,
            &|| false,
        );
        assert!(caps.hyperlinks);
        assert!(caps.true_color);
        // Unknown PI_IMAGE_PROTOCOL values leave detection alone.
        let caps = detect_capabilities_with(
            &env(&[("KITTY_WINDOW_ID", "1"), ("PI_IMAGE_PROTOCOL", "bogus")]),
            false,
            &|| false,
        );
        assert_eq!(caps.images, Some(ImageProtocol::Kitty));
    }

    #[test]
    fn capability_cache_and_overrides() {
        let _lock = global_test_lock();
        reset_for_test();
        set_capabilities(TerminalCapabilities {
            images: None,
            true_color: false,
            hyperlinks: false,
        });
        assert!(!get_capabilities().hyperlinks);
        set_capability_overrides(CapabilityOverrides {
            images: Some(None),
            true_color: Some(true),
            hyperlinks: Some(true),
        });
        assert!(get_capabilities().true_color);
        assert!(get_capabilities().hyperlinks);
        reset_capabilities_cache();
        // Overrides survive a cache reset (setCapabilityOverrides cleared it
        // already; the second read re-detects and re-applies).
        assert!(get_capabilities().hyperlinks);
        set_capability_overrides(CapabilityOverrides::default());
        reset_for_test();
    }

    #[test]
    fn cell_dimensions_roundtrip() {
        let _lock = global_test_lock();
        reset_for_test();
        assert_eq!(get_cell_dimensions(), CellDimensions { width_px: 9, height_px: 18 });
        set_cell_dimensions(14, 28);
        assert_eq!(get_cell_dimensions(), CellDimensions { width_px: 14, height_px: 28 });
        reset_for_test();
    }

    #[test]
    fn allocate_image_id_range_and_injection() {
        let _lock = global_test_lock();
        reset_for_test();
        for _ in 0..100 {
            let id = allocate_image_id();
            assert!((1..=0xFFFF_FFFF).contains(&id), "id out of range: {id}");
        }
        let mut counter = 0.0f64;
        set_image_id_random_source(Some(Box::new(move || {
            counter += 0.25;
            counter.min(0.999_999)
        })));
        assert_eq!(allocate_image_id(), (0.25 * 0xFFFF_FFFEu64 as f64).floor() as i64 + 1);
        reset_for_test();
    }

    #[test]
    fn delete_sequences() {
        assert_eq!(delete_kitty_image(42), "\x1b_Ga=d,d=I,i=42,q=2\x1b\\");
        assert_eq!(delete_all_kitty_images(), "\x1b_Ga=d,d=A,q=2\x1b\\");
        assert_eq!(delete_all_kitty_placements(), "\x1b_Ga=d,d=a,q=2\x1b\\");
    }

    fn metadata(image_id: i64, rows: i64, height_px: i64) -> KittyImageMetadata {
        KittyImageMetadata { image_id, columns: 10, rows, width_px: 90, height_px }
    }

    #[test]
    fn metadata_registry_lookup() {
        let _lock = global_test_lock();
        reset_for_test();
        let line = "\x1b_Ga=p,q=2,i=7,c=10,r=4;\x1b\\";
        assert_eq!(get_kitty_image_metadata(line), None);
        register_kitty_image_metadata(metadata(7, 4, 72));
        assert_eq!(get_kitty_image_metadata(line), Some(metadata(7, 4, 72)));
        // Re-registration replaces and bumps the generation.
        register_kitty_image_metadata(metadata(7, 6, 108));
        assert_eq!(get_kitty_image_metadata(line), Some(metadata(7, 6, 108)));
        assert_eq!(get_kitty_image_placement_rows(line), Some(4)); // explicit r=
        let no_rows = "\x1b_Ga=p,q=2,i=7,c=10;\x1b\\";
        assert_eq!(get_kitty_image_placement_rows(no_rows), Some(6)); // registry rows
        // Invalid digit forms do not match.
        assert_eq!(get_kitty_image_placement_rows("\x1b_Ga=p,i=7x;q=2\x1b\\"), None);
        reset_for_test();
    }

    #[test]
    fn metadata_registry_eviction() {
        let _lock = global_test_lock();
        reset_for_test();
        for id in 0..1001 {
            register_kitty_image_metadata(metadata(id, 1, 18));
        }
        // Entry 0 was evicted (oldest); 1..=1000 remain.
        assert_eq!(get_kitty_image_metadata("\x1b_Gi=0;q=2\x1b\\"), None);
        assert_eq!(get_kitty_image_metadata("\x1b_Gi=1;q=2\x1b\\"), Some(metadata(1, 1, 18)));
        assert_eq!(get_kitty_image_metadata("\x1b_Gi=1000;q=2\x1b\\"), Some(metadata(1000, 1, 18)));
        reset_for_test();
    }

    #[test]
    fn placement_single_chunk() {
        let _lock = global_test_lock();
        reset_for_test();
        register_kitty_image_metadata(metadata(9, 3, 54));
        let line = "prefix\x1b_Ga=T,i=9,c=10,r=3,m=0;DATA\x1b\\suffix";
        let placement = get_kitty_image_placement(line).expect("placement");
        assert_eq!(placement.image_id, 9);
        assert_eq!(placement.rows, 3);
        assert_eq!(placement.transmission_bytes, ("\x1b_Ga=T,i=9,c=10,r=3,m=0;DATA\x1b\\".len()) as i64);
        assert_eq!(placement.estimated_decoded_bytes, 90 * 54 * 4);
        assert_eq!(placement.sequence, "\x1b_Ga=p,q=2,i=9,c=10,r=3\x1b\\");
        assert_eq!(placement.replacement_line, "prefix\x1b_Ga=p,q=2,i=9,c=10,r=3\x1b\\suffix");
        assert_eq!(placement.transmission_generation, 1);
        reset_for_test();
    }

    #[test]
    fn placement_walks_chunked_transmission() {
        let _lock = global_test_lock();
        reset_for_test();
        register_kitty_image_metadata(metadata(5, 2, 36));
        let line = "x\x1b_Ga=T,i=5,m=1;AAAA\x1b\\\x1b_Gm=1;BBBB\x1b\\\x1b_Gm=0;CCCC\x1b\\y";
        let placement = get_kitty_image_placement(line).expect("placement");
        // The whole chunked transmission is replaced by the placement command.
        let expected_transmission = "\x1b_Ga=T,i=5,m=1;AAAA\x1b\\\x1b_Gm=1;BBBB\x1b\\\x1b_Gm=0;CCCC\x1b\\";
        assert_eq!(placement.transmission_bytes, expected_transmission.len() as i64);
        assert_eq!(placement.replacement_line, format!("x\x1b_Ga=p,q=2,i=5\x1b\\y"));
        // Unknown image ids produce no placement.
        assert_eq!(get_kitty_image_placement("x\x1b_Ga=T,i=99,m=0;D\x1b\\y"), None);
        // A dangling chunk (no terminator) produces no placement.
        assert_eq!(get_kitty_image_placement("\x1b_Ga=T,i=5,m=1;AAAA"), None);
        reset_for_test();
    }

    #[test]
    fn crop_kitty_image_rows() {
        let _lock = global_test_lock();
        reset_for_test();
        register_kitty_image_metadata(metadata(3, 4, 80));
        let line = "\x1b_Ga=T,i=3,c=10,r=4,y=0,h=80;DATA\x1b\\";
        // No metadata → unchanged.
        assert_eq!(crop_kitty_image_line("\x1b_Ga=T,i=77;D\x1b\\", 1, 2), "\x1b_Ga=T,i=77;D\x1b\\");
        // hidden=0, visible=all → unchanged.
        assert_eq!(crop_kitty_image_line(line, 0, 4), line);
        // Out-of-range hidden/visible → unchanged.
        assert_eq!(crop_kitty_image_line(line, 4, 1), line);
        assert_eq!(crop_kitty_image_line(line, 0, 0), line);
        // Crop rows 1-2 of 4 (height 80px): y=20, h=40, r=2.
        assert_eq!(
            crop_kitty_image_line(line, 1, 2),
            "\x1b_Ga=T,i=3,c=10,y=20,h=40,r=2;DATA\x1b\\"
        );
        // Requesting more rows than remain crops to the tail.
        assert_eq!(
            crop_kitty_image_line(line, 3, 10),
            "\x1b_Ga=T,i=3,c=10,y=60,h=20,r=1;DATA\x1b\\"
        );
        // All controls y/h/r only: rebuilt without a leading comma.
        let only_geometry = "\x1b_Gi=3,y=0,h=80,r=4;DATA\x1b\\";
        assert_eq!(
            crop_kitty_image_line(only_geometry, 1, 2),
            "\x1b_Gi=3,y=20,h=40,r=2;DATA\x1b\\"
        );
        reset_for_test();
    }

    // =========================================================================
    // T6: encoders, cell sizing, dimension parsers, renderImage, fallbacks
    // =========================================================================

    fn kitty_caps() {
        set_capabilities(TerminalCapabilities {
            images: Some(ImageProtocol::Kitty),
            true_color: true,
            hyperlinks: true,
        });
    }

    fn iterm2_caps() {
        set_capabilities(TerminalCapabilities {
            images: Some(ImageProtocol::Iterm2),
            true_color: true,
            hyperlinks: true,
        });
    }

    fn no_image_caps() {
        set_capabilities(TerminalCapabilities {
            images: None,
            true_color: false,
            hyperlinks: false,
        });
    }

    fn dims(width_px: i64, height_px: i64) -> ImageDimensions {
        ImageDimensions { width_px, height_px }
    }

    #[test]
    fn encode_kitty_small_payload() {
        // "can request no terminal-side cursor movement"
        let sequence = encode_kitty(
            "AAAA",
            &KittyEncodeOptions {
                columns: Some(2),
                rows: Some(2),
                move_cursor: Some(false),
                ..Default::default()
            },
        );
        assert_eq!(sequence, "\x1b_Ga=T,f=100,q=2,C=1,c=2,r=2;AAAA\x1b\\");
        // Default: terminal-side cursor movement on (no C=1).
        let sequence = encode_kitty("AAAA", &KittyEncodeOptions::default());
        assert_eq!(sequence, "\x1b_Ga=T,f=100,q=2;AAAA\x1b\\");
        // TS truthiness: Some(0) columns/rows/imageId are skipped.
        let sequence = encode_kitty(
            "AAAA",
            &KittyEncodeOptions {
                columns: Some(0),
                rows: Some(0),
                image_id: Some(0),
                ..Default::default()
            },
        );
        assert_eq!(sequence, "\x1b_Ga=T,f=100,q=2;AAAA\x1b\\");
    }

    #[test]
    fn encode_kitty_chunked_payload() {
        let payload = "A".repeat(4096 * 2 + 100);
        let sequence =
            encode_kitty(&payload, &KittyEncodeOptions { columns: Some(5), ..Default::default() });
        // First chunk: full params + m=1.
        assert!(sequence.starts_with("\x1b_Ga=T,f=100,q=2,c=5,m=1;"));
        // Middle chunk: m=1 without params. The first chunk is
        // "_G" + params + ";" (24 chars) + 4096 + "\" (2) = 4122.
        assert!(sequence[4123..].starts_with("\x1b_Gm=1;"));
        // Last chunk: m=0.
        assert!(sequence.contains("\x1b_Gm=0;"));
        // Exactly-4096 payload stays single-chunk.
        let single = encode_kitty(&"A".repeat(4096), &KittyEncodeOptions::default());
        assert_eq!(single, format!("\x1b_Ga=T,f=100,q=2;{}\x1b\\", "A".repeat(4096)));
    }

    #[test]
    fn encode_iterm2_metadata() {
        // "includes the decoded payload size in OSC 1337 metadata"
        let sequence = encode_iterm2(
            "AAAA",
            &Iterm2EncodeOptions {
                width: Some("2".into()),
                height: Some("auto".into()),
                ..Default::default()
            },
        );
        assert_eq!(sequence, "\x1b]1337;File=inline=1;size=3;width=2;height=auto:AAAA\x07");
        // name + preserveAspectRatio=0 + inline=0.
        let sequence = encode_iterm2(
            "AAAA",
            &Iterm2EncodeOptions {
                name: Some("pic.png".into()),
                preserve_aspect_ratio: Some(false),
                inline: Some(false),
                ..Default::default()
            },
        );
        assert_eq!(
            sequence,
            format!(
                "\x1b]1337;File=inline=0;size=3;name={};preserveAspectRatio=0:AAAA\x07",
                base64_encode(b"pic.png")
            )
        );
    }

    #[test]
    fn base64_node_semantics() {
        assert_eq!(base64_decode("AAAA"), vec![0, 0, 0]);
        assert_eq!(base64_byte_length("AAAA"), 3);
        // Lenient: padding and whitespace skipped.
        assert_eq!(base64_byte_length("AAAA====="), 3);
        assert_eq!(base64_byte_length("AA AA\n"), 3);
        // URL-safe alphabet accepted.
        assert_eq!(base64_decode("-_8="), base64_decode("+/8"));
        // 2 leftover chars → 1 byte; 1 leftover char → 0 bytes.
        assert_eq!(base64_byte_length("AA"), 1);
        assert_eq!(base64_byte_length("A"), 0);
        assert_eq!(base64_encode(b"pic.png"), "cGljLnBuZw==");
        assert_eq!(base64_decode("cGljLnBuZw=="), b"pic.png".to_vec());
    }

    #[test]
    fn render_image_kitty_default_cursor_movement() {
        let _lock = global_test_lock();
        reset_for_test();
        kitty_caps();
        set_cell_dimensions(10, 10);
        let result = render_image(
            "AAAA",
            dims(20, 20),
            &ImageRenderOptions { max_width_cells: Some(2), ..Default::default() },
        )
        .expect("kitty render");
        assert!(!result.sequence.contains(",C=1,"));
        assert_eq!(result.rows, 2);
        // "can opt renderImage into no terminal-side cursor movement"
        let result = render_image(
            "AAAA",
            dims(20, 20),
            &ImageRenderOptions {
                max_width_cells: Some(2),
                move_cursor: Some(false),
                ..Default::default()
            },
        )
        .expect("kitty render");
        assert!(result.sequence.contains(",C=1,"));
        assert_eq!(result.rows, 2);
        reset_for_test();
    }

    #[test]
    fn render_image_kitty_thin_image_single_row() {
        let _lock = global_test_lock();
        reset_for_test();
        kitty_caps();
        set_cell_dimensions(9, 18);
        let result = render_image(
            "AAAA",
            dims(1200, 12),
            &ImageRenderOptions { max_width_cells: Some(60), ..Default::default() },
        )
        .expect("kitty render");
        assert_eq!(result.rows, 1);
        assert!(result.sequence.contains(",c=60,r=1;"));
        reset_for_test();
    }

    #[test]
    fn render_image_kitty_ceiling_placement() {
        let _lock = global_test_lock();
        reset_for_test();
        kitty_caps();
        set_cell_dimensions(15, 28);
        let result = render_image(
            "AAAA",
            dims(615, 86),
            &ImageRenderOptions { max_width_cells: Some(60), ..Default::default() },
        )
        .expect("kitty render");
        assert_eq!(result.rows, 5);
        assert!(result.sequence.contains(",c=60,r=5;"));
        reset_for_test();
    }

    #[test]
    fn render_image_kitty_metadata_and_crop() {
        let _lock = global_test_lock();
        reset_for_test();
        kitty_caps();
        set_cell_dimensions(14, 28);
        let result = render_image(
            "AAAA",
            dims(400, 900),
            // max_height_cells: the TS test reaches renderImage through the
            // Image component, which caps height to a square pixel box:
            // floor(maxWidthCells * cellW / cellH) = floor(30 * 14 / 28) = 15.
            &ImageRenderOptions {
                max_width_cells: Some(30),
                max_height_cells: Some(15),
                image_id: Some(8938),
                // The Image component passes moveCursor: false; the C=1 it
                // adds survives into the placement-only command.
                move_cursor: Some(false),
                ..Default::default()
            },
        )
        .expect("kitty render");
        assert!(result.sequence.contains(",c=13,r=15,i=8938;"));
        assert_eq!(
            get_kitty_image_metadata(&result.sequence),
            Some(KittyImageMetadata {
                image_id: 8938,
                columns: 13,
                rows: 15,
                width_px: 400,
                height_px: 900,
            })
        );
        let placement = get_kitty_image_placement(&crop_kitty_image_line(&result.sequence, 1, 2))
            .expect("cropped placement");
        assert_eq!(placement.sequence, "\x1b_Ga=p,q=2,C=1,c=13,i=8938,y=60,h=120,r=2\x1b\\");
        reset_for_test();
    }

    #[test]
    fn render_image_kitty_thin_widths_by_proportion() {
        let _lock = global_test_lock();
        reset_for_test();
        kitty_caps();
        set_cell_dimensions(1, 1);
        for (width_px, columns) in [(1, 1), (140, 1), (149, 2)] {
            let result = render_image(
                "AAAA",
                dims(width_px, 1000),
                &ImageRenderOptions {
                    max_width_cells: Some(30),
                    max_height_cells: Some(10),
                    ..Default::default()
                },
            )
            .expect("kitty render");
            assert_eq!(result.columns, columns);
            assert_eq!(result.rows, 10);
            assert!(result.sequence.contains(&format!(",c={columns},r=10;")));
        }
        reset_for_test();
    }

    #[test]
    fn render_image_iterm2_ceiling_width() {
        let _lock = global_test_lock();
        reset_for_test();
        iterm2_caps();
        set_cell_dimensions(14, 28);
        let result = render_image(
            "AAAA",
            dims(400, 900),
            &ImageRenderOptions {
                max_width_cells: Some(30),
                max_height_cells: Some(15),
                ..Default::default()
            },
        )
        .expect("iterm2 render");
        assert_eq!(result.columns, 14);
        assert_eq!(result.rows, 15);
        assert_eq!(result.sequence, "\x1b]1337;File=inline=1;size=3;width=14;height=auto:AAAA\x07");
        reset_for_test();
    }

    #[test]
    fn render_image_without_protocol() {
        let _lock = global_test_lock();
        reset_for_test();
        no_image_caps();
        assert!(render_image("AAAA", dims(20, 20), &ImageRenderOptions::default()).is_none());
        reset_for_test();
    }

    #[test]
    fn image_dimension_parsers() {
        // PNG: signature + IHDR width/height at offsets 16/20.
        let mut png = vec![0u8; 24];
        png[0..4].copy_from_slice(&[0x89, 0x50, 0x4E, 0x47]);
        png[16..20].copy_from_slice(&615u32.to_be_bytes());
        png[20..24].copy_from_slice(&86u32.to_be_bytes());
        let png_b64 = base64_encode(&png);
        assert_eq!(get_png_dimensions(&png_b64), Some(dims(615, 86)));
        assert_eq!(get_image_dimensions(&png_b64, "image/png"), Some(dims(615, 86)));
        // Too short / wrong signature.
        assert_eq!(get_png_dimensions("AAAA"), None);
        let mut bad = png.clone();
        bad[0] = 0x00;
        assert_eq!(get_png_dimensions(&base64_encode(&bad)), None);

        // JPEG stub always returns null.
        assert_eq!(get_jpeg_dimensions(&png_b64), None);
        assert_eq!(get_image_dimensions(&png_b64, "image/jpeg"), None);

        // GIF: signature + LE u16 at 6/8.
        let mut gif = vec![0u8; 10];
        gif[0..6].copy_from_slice(b"GIF89a");
        gif[6..8].copy_from_slice(&640u16.to_le_bytes());
        gif[8..10].copy_from_slice(&480u16.to_le_bytes());
        let gif_b64 = base64_encode(&gif);
        assert_eq!(get_gif_dimensions(&gif_b64), Some(dims(640, 480)));
        assert_eq!(get_image_dimensions(&gif_b64, "image/gif"), Some(dims(640, 480)));

        // WebP VP8L: bits at 21 encode (width-1) and (height-1).
        let mut webp = vec![0u8; 30];
        webp[0..4].copy_from_slice(b"RIFF");
        webp[8..12].copy_from_slice(b"WEBP");
        webp[12..16].copy_from_slice(b"VP8L");
        let bits = (((480u32) - 1) << 14) | ((640u32) - 1);
        webp[21..25].copy_from_slice(&bits.to_le_bytes());
        let webp_b64 = base64_encode(&webp);
        assert_eq!(get_webp_dimensions(&webp_b64), Some(dims(640, 480)));
        assert_eq!(get_image_dimensions(&webp_b64, "image/webp"), Some(dims(640, 480)));

        // WebP VP8X: 24-bit canvas size - 1 at 24/27.
        webp[12..16].copy_from_slice(b"VP8X");
        webp[24..27].copy_from_slice(&[99, 0, 0]); // width 100
        webp[27..30].copy_from_slice(&[49, 0, 0]); // height 50
        let webp_b64 = base64_encode(&webp);
        assert_eq!(get_webp_dimensions(&webp_b64), Some(dims(100, 50)));

        // Unknown MIME type.
        assert_eq!(get_image_dimensions(&png_b64, "image/bmp"), None);
    }

    #[test]
    fn calculate_cell_size_math() {
        let cells = CellDimensions { width_px: 9, height_px: 18 };
        // No height cap: scale by width.
        let size = calculate_image_cell_size(dims(90, 180), 10, None, cells, false);
        assert_eq!(size, ImageCellSize { columns: 10, rows: 10 });
        // Height cap limits rows and width together (uniform scale).
        let size = calculate_image_cell_size(dims(90, 180), 10, Some(5), cells, false);
        assert_eq!(size, ImageCellSize { columns: 5, rows: 5 });
        // calculateImageRows delegates without aspect optimization.
        assert_eq!(calculate_image_rows(dims(90, 180), 10, cells), 10);
    }

    #[test]
    fn hyperlink_osc8() {
        assert_eq!(
            hyperlink("text", "https://example.com"),
            "\x1b]8;;https://example.com\x1b\\text\x1b]8;;\x1b\\"
        );
        assert_eq!(hyperlink("", "file:///x"), "\x1b]8;;file:///x\x1b\\\x1b]8;;\x1b\\");
    }

    #[test]
    fn image_fallback_variants() {
        let _lock = global_test_lock();
        reset_for_test();
        no_image_caps();
        // No filename / no dimensions.
        assert_eq!(image_fallback("image/png", None, None), "[Image: [image/png]]");
        // Dimensions only.
        assert_eq!(image_fallback("image/png", Some(dims(100, 50)), None), "[Image: [image/png] 100x50]");
        // Relative basename: never hyperlinked.
        assert_eq!(
            image_fallback("image/png", Some(dims(100, 50)), Some("pic.png")),
            "[Image: pic.png [image/png] 100x50]"
        );
        // Absolute path without hyperlink support: shortened but plain.
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            let path = format!("{home}/docs/pic.png");
            assert_eq!(image_fallback("image/png", None, Some(&path)), "[Image: ~/docs/pic.png [image/png]]");
        }
        // Hyperlinks enabled: absolute path wrapped in OSC 8 file link.
        set_capabilities(TerminalCapabilities { images: None, true_color: true, hyperlinks: true });
        if !home.is_empty() {
            let path = format!("{home}/docs/pic.png");
            let expected = format!(
                "[Image: \x1b]8;;file://{home}/docs/pic.png\x1b\\~/docs/pic.png\x1b]8;;\x1b\\ [image/png]]"
            );
            assert_eq!(image_fallback("image/png", None, Some(&path)), expected);
        }
        reset_for_test();
    }
}
