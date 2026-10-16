//! pi-cli: CLI entry point for the Rust port of the pi coding agent.
//!
//! Subcommands:
//! - `harness tool <name>`: execute a built-in tool with JSON args from
//!   stdin, print a JSON result on stdout. Used by the parity harness.
//! - `harness pure <fn>`: invoke pure pi-core functions with JSON args from
//!   stdin, print JSON result. Used by the parity harness.

use std::io::Read;

use serde_json::Value;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = run(&args);
    match result {
        Ok(value) => {
            println!("{}", serde_json::to_string_pretty(&value).unwrap());
        }
        Err(message) => {
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "error": message })).unwrap());
            std::process::exit(1);
        }
    }
}

fn read_stdin_json() -> Result<Value, String> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| format!("failed to read stdin: {}", e))?;
    if input.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(&input).map_err(|e| format!("invalid JSON on stdin: {}", e))
}

fn run(args: &[String]) -> Result<Value, String> {
    match args.first().map(|s| s.as_str()) {
        Some("harness") => run_harness(&args[1..]),
        _ => {
            eprintln!("pi-cli (Rust port of pi coding agent)");
            eprintln!();
            eprintln!("Usage:");
            eprintln!("  pi-cli harness tool <name> [--cwd <dir>]      run a tool; JSON args on stdin");
            eprintln!("  pi-cli harness pure <fn> [--cwd <dir>]        run a pure function; JSON args on stdin");
            eprintln!();
            eprintln!("Tools: read, write, edit, ls, grep, find, bash");
            Err("missing subcommand".to_string())
        }
    }
}

fn run_harness(args: &[String]) -> Result<Value, String> {
    let mode = args.first().ok_or("missing harness mode (tool|pure)")?;
    let rest = &args[1..];

    // Parse --cwd
    let mut cwd = std::env::current_dir()
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .into_owned();
    let mut i = 0;
    let mut positional: Vec<String> = Vec::new();
    while i < rest.len() {
        if rest[i] == "--cwd" {
            cwd = rest
                .get(i + 1)
                .ok_or("--cwd requires a value")?
                .clone();
            i += 2;
        } else {
            positional.push(rest[i].clone());
            i += 1;
        }
    }

    let name = positional
        .first()
        .ok_or_else(|| format!("missing {} name", mode))?
        .clone();

    match mode.as_str() {
        "tool" => run_tool(&name, &cwd),
        "pure" => run_pure(&name, &cwd),
        _ => Err(format!("unknown harness mode: {}", mode)),
    }
}

fn run_tool(name: &str, cwd: &str) -> Result<Value, String> {
    let input = read_stdin_json()?;
    // Module-level harness entries (not AgentTools): the bash executor and
    // the output accumulator, driven directly.
    match name {
        "bash-executor" => return run_bash_executor_harness(&input, cwd),
        "output-accumulator" => return run_output_accumulator_harness(&input),
        "messages" => return run_messages_harness(&input),
        "session-manager" => return run_session_manager_harness(&input, cwd),
        "compaction-utils" => return run_compaction_utils_harness(&input),
        "compaction" => return run_compaction_harness(&input),
        "nested-calls" => return run_nested_calls_harness(&input),
        "tui-utils" => return run_tui_utils_harness(&input),
        "tui-fuzzy" => return run_tui_fuzzy_harness(&input),
        "tui-kill-ring" => return run_tui_kill_ring_harness(&input),
        "tui-program-status" => return run_tui_program_status_harness(&input),
        "tui-colors" => return run_tui_colors_harness(&input),
        "tui-terminal-colors" => return run_tui_terminal_colors_harness(&input),
        "math" => return run_math_harness(&input),
        _ => {}
    }
    let result: Result<pi_tools::ToolOutput, String> = match name {
        "read" => serde_json::from_value::<pi_tools::read::ReadToolInput>(input)
            .map_err(|e| e.to_string())
            .and_then(|args| pi_tools::read::read_execute(cwd, &args).map_err(|e| e.to_string())),
        "write" => serde_json::from_value::<pi_tools::write::WriteToolInput>(input)
            .map_err(|e| e.to_string())
            .and_then(|args| pi_tools::write::write_execute(cwd, &args).map_err(|e| e.to_string())),
        "edit" => {
            let mut value = input;
            pi_tools::edit::prepare_edit_arguments(&mut value);
            serde_json::from_value::<pi_tools::edit::EditToolInput>(value)
                .map_err(|e| e.to_string())
                .and_then(|args| pi_tools::edit::edit_execute(cwd, &args).map_err(|e| e.to_string()))
        }
        "ls" => serde_json::from_value::<pi_tools::ls::LsToolInput>(input)
            .map_err(|e| e.to_string())
            .and_then(|args| pi_tools::ls::ls_execute(cwd, &args).map_err(|e| e.to_string())),
        "grep" => serde_json::from_value::<pi_tools::grep::GrepToolInput>(input)
            .map_err(|e| e.to_string())
            .and_then(|args| pi_tools::grep::grep_execute(cwd, &args).map_err(|e| e.to_string())),
        "find" => serde_json::from_value::<pi_tools::find::FindToolInput>(input)
            .map_err(|e| e.to_string())
            .and_then(|args| pi_tools::find::find_execute(cwd, &args).map_err(|e| e.to_string())),
        "bash" => serde_json::from_value::<pi_tools::bash::BashToolInput>(input)
            .map_err(|e| e.to_string())
            .and_then(|args| pi_tools::bash::bash_execute(cwd, &args).map_err(|e| e.to_string())),
        other => return Err(format!("unknown tool: {}", other)),
    };

    match result {
        Ok(output) => Ok(serde_json::to_value(&output).map_err(|e| e.to_string())?),
        Err(err) => Ok(serde_json::json!({ "error": err })),
    }
}

/// Drive pi-core `messages.rs` functions (mirrors dist/core/messages.js).
/// `tui-utils` module-level harness entry: drives the pure functions of
/// `packages/tui/src/utils.ts` (mirrored on the TS side by ts-harness.js).
fn run_tui_utils_harness(input: &Value) -> Result<Value, String> {
    use pi_tui::utils;
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    let text = |key: &str| -> Result<&str, String> {
        input[key]
            .as_str()
            .ok_or_else(|| format!("missing {key}"))
    };
    let num = |key: &str| -> Result<usize, String> {
        input[key]
            .as_u64()
            .map(|v| v as usize)
            .ok_or_else(|| format!("missing {key}"))
    };
    let opt_bool = |key: &str, default: bool| -> bool {
        input[key].as_bool().unwrap_or(default)
    };
    match fn_name {
        "visibleWidth" => Ok(serde_json::json!({ "result": utils::visible_width(text("text")?) })),
        "stripTerminalSequences" => Ok(serde_json::json!({ "result": utils::strip_terminal_sequences(text("text")?) })),
        "normalizeTerminalOutput" => Ok(serde_json::json!({ "result": utils::normalize_terminal_output(text("text")?) })),
        "getActiveBackgroundAnsi" => Ok(serde_json::json!({ "result": utils::get_active_background_ansi(text("text")?) })),
        "getGraphemeCellRange" => {
            let range = utils::get_grapheme_cell_range(text("line")?, num("column")?);
            Ok(match range {
                Some(r) => serde_json::json!({ "result": { "start": r.start, "end": r.end } }),
                None => serde_json::json!({ "result": null }),
            })
        }
        "getOsc8LinkAtColumn" => {
            let link = utils::get_osc8_link_at_column(text("line")?, num("column")?);
            Ok(serde_json::json!({ "result": link }))
        }
        "truncateToWidth" => {
            let ellipsis = input["ellipsis"].as_str().unwrap_or("...");
            let pad = opt_bool("pad", false);
            Ok(serde_json::json!({
                "result": utils::truncate_to_width(text("text")?, num("maxWidth")?, ellipsis, pad)
            }))
        }
        "sliceByColumn" => {
            let strict = opt_bool("strict", false);
            Ok(serde_json::json!({
                "result": utils::slice_by_column(text("line")?, num("startCol")?, num("length")?, strict)
            }))
        }
        "sliceWithWidth" => {
            let strict = opt_bool("strict", false);
            let r = utils::slice_with_width(text("line")?, num("startCol")?, num("length")?, strict);
            Ok(serde_json::json!({ "result": { "text": r.text, "width": r.width } }))
        }
        "extractSegments" => {
            let strict_after = opt_bool("strictAfter", false);
            let r = utils::extract_segments(
                text("line")?,
                num("beforeEnd")?,
                num("afterStart")?,
                num("afterLen")?,
                strict_after,
            );
            Ok(serde_json::json!({
                "result": {
                    "before": r.before, "beforeWidth": r.before_width,
                    "after": r.after, "afterWidth": r.after_width
                }
            }))
        }
        "wrapTextWithAnsi" => {
            Ok(serde_json::json!({ "result": utils::wrap_text_with_ansi(text("text")?, num("width")?) }))
        }
        "isWhitespaceChar" => {
            let c = text("char")?.chars().next().ok_or("empty char")?;
            Ok(serde_json::json!({ "result": utils::is_whitespace_char(c) }))
        }
        "isPunctuationChar" => {
            let c = text("char")?.chars().next().ok_or("empty char")?;
            Ok(serde_json::json!({ "result": utils::is_punctuation_char(c) }))
        }
        other => Err(format!("unknown tui-utils fn: {}", other)),
    }
}
// --- TUI module harness entries (tui-fuzzy, tui-kill-ring, --------------------
// --- tui-program-status, tui-colors, tui-terminal-colors) ---------------------

/// Serialize an f64 the way JS `JSON.stringify` would (integral values lose
/// the trailing `.0` that serde_json keeps).
fn js_num(x: f64) -> Value {
    if x.is_finite() && x.fract() == 0.0 && x.abs() <= 9_007_199_254_740_992.0 {
        serde_json::json!(x as i64)
    } else {
        serde_json::json!(x)
    }
}

/// `tui-fuzzy` entry: `fuzzyMatch` and `fuzzyFilter`.
fn run_tui_fuzzy_harness(input: &Value) -> Result<Value, String> {
    use pi_tui::fuzzy;
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    match fn_name {
        "fuzzyMatch" => {
            let m = fuzzy::fuzzy_match(
                input["query"].as_str().unwrap_or(""),
                input["text"].as_str().unwrap_or(""),
            );
            Ok(serde_json::json!({
                "result": { "matches": m.matches, "score": js_num(m.score) }
            }))
        }
        "fuzzyFilter" => {
            let items = input["items"].as_array().ok_or("missing items array")?;
            let query = input["query"].as_str().ok_or("missing query")?;
            let text_field = input["textField"].as_str();
            let get_text = |item: &Value| -> Result<String, String> {
                match text_field {
                    Some(field) => item[field]
                        .as_str()
                        .map(|s| s.to_string())
                        .ok_or_else(|| format!("missing field {field}")),
                    None => item
                        .as_str()
                        .map(|s| s.to_string())
                        .ok_or_else(|| "item is not a string".to_string()),
                }
            };
            let texts: Vec<String> = items.iter().map(get_text).collect::<Result<_, _>>()?;
            // Filter over indices so the original items are returned (JS
            // returns the item objects themselves).
            let indexed: Vec<(usize, String)> =
                texts.iter().cloned().enumerate().collect();
            let filtered = fuzzy::fuzzy_filter(&indexed, query, |(_, t)| t.as_str());
            let out: Vec<Value> = filtered.iter().map(|(i, _)| items[*i].clone()).collect();
            Ok(serde_json::json!({ "result": out }))
        }
        other => Err(format!("unknown tui-fuzzy fn: {}", other)),
    }
}

/// `tui-kill-ring` entry: a sequence of ring ops, returning the observable
/// results (peek values, lengths) in order.
fn run_tui_kill_ring_harness(input: &Value) -> Result<Value, String> {
    use pi_tui::kill_ring::KillRing;
    let ops = input["ops"].as_array().ok_or("missing ops array")?;
    let mut ring = KillRing::new();
    let mut results = Vec::new();
    for op in ops {
        let name = op["op"].as_str().ok_or("missing op name")?;
        match name {
            "push" => {
                ring.push(
                    op["text"].as_str().unwrap_or(""),
                    op["prepend"].as_bool().unwrap_or(false),
                    op["accumulate"].as_bool().unwrap_or(false),
                );
            }
            "peek" => results.push(match ring.peek() {
                Some(s) => serde_json::json!(s),
                None => serde_json::json!(null),
            }),
            "rotate" => ring.rotate(),
            "length" => results.push(serde_json::json!(ring.len())),
            other => return Err(format!("unknown kill-ring op: {}", other)),
        }
    }
    Ok(serde_json::json!({ "result": results }))
}

/// `tui-program-status` entry.
fn run_tui_program_status_harness(input: &Value) -> Result<Value, String> {
    use pi_tui::program_status;
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    match fn_name {
        "formatProgramStatus" => {
            let status = program_status::ProgramStatus {
                state: input["state"]
                    .as_str()
                    .and_then(program_status::ProgramState::parse),
                app: input["app"].as_str().map(|s| s.to_string()),
                kind: input["kind"]
                    .as_str()
                    .and_then(program_status::ProgramStatusKind::parse),
                message: input["message"].as_str().map(|s| s.to_string()),
            };
            Ok(serde_json::json!({
                "result": program_status::format_program_status(&status)
            }))
        }
        "isProgramStatusReply" => Ok(serde_json::json!({
            "result": program_status::is_program_status_reply(
                input["sequence"].as_str().unwrap_or("")
            )
        })),
        other => Err(format!("unknown tui-program-status fn: {}", other)),
    }
}

/// Parse a harness `color` argument: a string (parsed), a number (indexed),
/// or an already-constructed color object.
fn color_from_value(v: &Value) -> Result<pi_tui::colors::Color, String> {
    use pi_tui::colors::{indexed_color, oklch_color, parse_color, Color};
    match v {
        Value::String(s) => parse_color(s),
        Value::Number(n) => indexed_color(n.as_f64().unwrap_or(f64::NAN)),
        Value::Object(o) => match o["kind"].as_str() {
            Some("indexed") => indexed_color(o["index"].as_f64().unwrap_or(f64::NAN)),
            Some("rgb") => Ok(Color::Rgb {
                r: o["r"].as_f64().unwrap_or(0.0),
                g: o["g"].as_f64().unwrap_or(0.0),
                b: o["b"].as_f64().unwrap_or(0.0),
            }),
            Some("oklch") => oklch_color(
                o["l"].as_f64().unwrap_or(0.0),
                o["c"].as_f64().unwrap_or(0.0),
                o["h"].as_f64().unwrap_or(0.0),
            ),
            _ => Err("Invalid color value: <object without kind>".to_string()),
        },
        _ => Err("Invalid color value: <non-string>".to_string()),
    }
}

/// Serialize a color like the TS object shape.
fn color_to_value(c: pi_tui::colors::Color) -> Value {
    use pi_tui::colors::Color;
    match c {
        Color::Indexed { index } => serde_json::json!({ "kind": "indexed", "index": index }),
        Color::Rgb { r, g, b } => serde_json::json!({
            "kind": "rgb", "r": js_num(r), "g": js_num(g), "b": js_num(b)
        }),
        Color::Oklch { l, c, h } => serde_json::json!({
            "kind": "oklch", "l": js_num(l), "c": js_num(c), "h": js_num(h)
        }),
    }
}

/// `tui-colors` entry. Expected errors are reported in-band as
/// `{ "error": message }` so both sides compare identical messages (the TS
/// harness catches throws and returns the same shape).
fn run_tui_colors_harness(input: &Value) -> Result<Value, String> {
    use pi_tui::colors::{
        background_ansi, color_to_hex, color_to_oklch, color_to_okhsl, color_to_rgb,
        foreground_ansi, indexed_color, mix_colors, okhsl_color, oklch_color, parse_color,
        rgb_color, style_text, style_text_with_ansi, ColorMixSpace, TerminalColorMode,
        TextAttributes, TextStyle,
    };
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    let num = |key: &str| input[key].as_f64().unwrap_or(f64::NAN);
    let mode = |key: &str| -> Result<TerminalColorMode, String> {
        TerminalColorMode::parse(input[key].as_str().unwrap_or(""))
            .ok_or_else(|| "unknown color mode".to_string())
    };

    let run = || -> Result<Value, String> {
        match fn_name {
            "parseColor" => match &input["value"] {
                Value::String(s) => parse_color(s).map(color_to_value),
                Value::Number(n) => {
                    indexed_color(n.as_f64().unwrap_or(f64::NAN)).map(color_to_value)
                }
                _ => Err("Invalid color value: <non-string>".to_string()),
            },
            "indexedColor" => indexed_color(num("index")).map(color_to_value),
            "rgbColor" => rgb_color(num("r"), num("g"), num("b")).map(color_to_value),
            "oklchColor" => oklch_color(num("l"), num("c"), num("h")).map(color_to_value),
            "okhslColor" => okhsl_color(num("h"), num("s"), num("l")).map(color_to_value),
            "colorToRgb" => color_from_value(&input["color"]).map(|c| {
                let rgb = color_to_rgb(c);
                serde_json::json!({ "r": js_num(rgb.r), "g": js_num(rgb.g), "b": js_num(rgb.b) })
            }),
            "colorToOklch" => color_from_value(&input["color"]).map(|c| {
                let o = color_to_oklch(c);
                serde_json::json!({ "l": js_num(o.l), "c": js_num(o.c), "h": js_num(o.h) })
            }),
            "colorToOkhsl" => color_from_value(&input["color"]).map(|c| {
                let o = color_to_okhsl(c);
                serde_json::json!({ "h": js_num(o.h), "s": js_num(o.s), "l": js_num(o.l) })
            }),
            "colorToHex" => color_from_value(&input["color"]).map(|c| serde_json::json!(color_to_hex(c))),
            "mixColors" => {
                let space = if input["space"].as_str() == Some("srgb") {
                    ColorMixSpace::Srgb
                } else {
                    ColorMixSpace::Oklch
                };
                color_from_value(&input["first"]).and_then(|first| {
                    color_from_value(&input["second"]).and_then(|second| {
                        mix_colors(first, second, num("amount"), space).map(color_to_value)
                    })
                })
            }
            "foregroundAnsi" => color_from_value(&input["color"])
                .and_then(|c| mode("mode").map(|m| serde_json::json!(foreground_ansi(c, m)))),
            "backgroundAnsi" => color_from_value(&input["color"])
                .and_then(|c| mode("mode").map(|m| serde_json::json!(background_ansi(c, m)))),
            "styleText" => {
                let style = &input["style"];
                // Mirror the TS path exactly: styleText passes options.fg/bg
                // straight to foregroundAnsi/backgroundAnsi without parsing.
                // A truthy non-Color value reaches colorToRgb, falls through
                // the `switch (color.kind)`, and the `{ r, g, b }`
                // destructuring throws the V8 TypeError below. Falsy values
                // (null, "", 0, false) are skipped by `options.fg && ...`.
                const TYPE_ERROR: &str = "Cannot read properties of undefined (reading 'r')";
                let style_color = |value: &Value| -> Result<Option<pi_tui::colors::Color>, String> {
                    if value.is_null() {
                        return Ok(None);
                    }
                    let truthy = match value {
                        Value::String(s) => !s.is_empty(),
                        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
                        Value::Bool(b) => *b,
                        _ => true,
                    };
                    if !truthy {
                        return Ok(None);
                    }
                    match value {
                        Value::Object(o) => match o["kind"].as_str() {
                            Some("indexed") => indexed_color(o["index"].as_f64().unwrap_or(f64::NAN)).map(Some),
                            Some("rgb") => Ok(Some(pi_tui::colors::Color::Rgb {
                                r: o["r"].as_f64().unwrap_or(f64::NAN),
                                g: o["g"].as_f64().unwrap_or(f64::NAN),
                                b: o["b"].as_f64().unwrap_or(f64::NAN),
                            })),
                            Some("oklch") => oklch_color(
                                o["l"].as_f64().unwrap_or(f64::NAN),
                                o["c"].as_f64().unwrap_or(f64::NAN),
                                o["h"].as_f64().unwrap_or(f64::NAN),
                            ).map(Some),
                            _ => Err(TYPE_ERROR.to_string()),
                        },
                        _ => Err(TYPE_ERROR.to_string()),
                    }
                };
                let fg = style_color(&style["fg"])?;
                let bg = style_color(&style["bg"])?;
                let options = TextStyle {
                    fg,
                    bg,
                    bold: style["bold"].as_bool().unwrap_or(false),
                    dim: style["dim"].as_bool().unwrap_or(false),
                    italic: style["italic"].as_bool().unwrap_or(false),
                    underline: style["underline"].as_bool().unwrap_or(false),
                    inverse: style["inverse"].as_bool().unwrap_or(false),
                    strikethrough: style["strikethrough"].as_bool().unwrap_or(false),
                };
                let m = mode("mode")?;
                Ok(serde_json::json!(style_text(
                    input["text"].as_str().unwrap_or(""),
                    &options,
                    m
                )))
            }
            "styleTextWithAnsi" => {
                let attrs = &input["options"];
                let options = TextAttributes {
                    bold: attrs["bold"].as_bool().unwrap_or(false),
                    dim: attrs["dim"].as_bool().unwrap_or(false),
                    italic: attrs["italic"].as_bool().unwrap_or(false),
                    underline: attrs["underline"].as_bool().unwrap_or(false),
                    inverse: attrs["inverse"].as_bool().unwrap_or(false),
                    strikethrough: attrs["strikethrough"].as_bool().unwrap_or(false),
                };
                let fg = input["fgAnsi"].as_str();
                let bg = input["bgAnsi"].as_str();
                Ok(serde_json::json!(style_text_with_ansi(
                    input["text"].as_str().unwrap_or(""),
                    fg,
                    bg,
                    &options,
                )))
            }
            other => Err(format!("unknown tui-colors fn: {}", other)),
        }
    };

    match run() {
        Ok(value) => Ok(serde_json::json!({ "result": value })),
        // Unknown-fn errors stay hard errors; everything else is in-band.
        Err(message) if message.starts_with("unknown tui-colors fn") => Err(message),
        Err(message) => Ok(serde_json::json!({ "error": message })),
    }
}

/// `math` entry: batch bit-level evaluation of `Math.*` functions for
/// differential float testing. Input: { ops: [{ fn, args: [..] }] }.
/// Output: { results: ["<16 hex chars of f64 bits>", ...] }.
fn run_math_harness(input: &Value) -> Result<Value, String> {
    let ops = input["ops"].as_array().ok_or("missing ops")?;
    let mut results = Vec::with_capacity(ops.len());
    for op in ops {
        let fn_name = op["fn"].as_str().ok_or("missing fn")?;
        let args: Vec<f64> = op["args"]
            .as_array()
            .ok_or_else(|| format!("missing args for fn {}", fn_name))?
            .iter()
            .map(|v| {
                v.as_f64()
                    .ok_or_else(|| format!("non-numeric arg for fn {}", fn_name))
            })
            .collect::<Result<_, _>>()?;
        let value = match (fn_name, args.as_slice()) {
            ("atan2", [y, x]) => pi_tui::fdlibm::atan2(*y, *x),
            ("sin", [x]) => pi_tui::fdlibm::sin(*x),
            ("cos", [x]) => pi_tui::fdlibm::cos(*x),
            ("cbrt", [x]) => pi_tui::fdlibm::cbrt(*x),
            ("hypot", [a, b]) => pi_tui::fdlibm::hypot(*a, *b),
            ("sqrt", [x]) => x.sqrt(),
            ("exp", [x]) => x.exp(),
            ("pow", [x, y]) => x.powf(*y),
            ("log", [x]) => x.ln(),
            ("atan", [x]) => x.atan(),
            _ => return Err(format!("unknown math fn: {}", fn_name)),
        };
        results.push(format!("{:016x}", value.to_bits()));
    }
    Ok(serde_json::json!({ "results": results }))
}
fn run_tui_terminal_colors_harness(input: &Value) -> Result<Value, String> {
    use pi_tui::terminal_colors::{
        parse_osc_color_response, parse_terminal_color_scheme_report, OscColorTarget,
        TerminalColorScheme,
    };
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    match fn_name {
        "parseOscColorResponse" => {
            let data = input["data"].as_str().unwrap_or("");
            let result = match parse_osc_color_response(data) {
                None => serde_json::json!(null),
                Some((target, rgb)) => {
                    let target = match target {
                        OscColorTarget::Foreground => serde_json::json!("foreground"),
                        OscColorTarget::Background => serde_json::json!("background"),
                        OscColorTarget::Index(i) => serde_json::json!(i),
                    };
                    let rgb = match rgb {
                        Some(c) => serde_json::json!({
                            "r": js_num(c.r), "g": js_num(c.g), "b": js_num(c.b)
                        }),
                        None => serde_json::json!(null),
                    };
                    serde_json::json!({ "target": target, "rgb": rgb })
                }
            };
            Ok(serde_json::json!({ "result": result }))
        }
        "parseTerminalColorSchemeReport" => {
            let data = input["data"].as_str().unwrap_or("");
            let result = match parse_terminal_color_scheme_report(data) {
                Some(TerminalColorScheme::Dark) => serde_json::json!("dark"),
                Some(TerminalColorScheme::Light) => serde_json::json!("light"),
                None => serde_json::json!(null),
            };
            Ok(serde_json::json!({ "result": result }))
        }
        other => Err(format!("unknown tui-terminal-colors fn: {}", other)),
    }
}

fn run_messages_harness(input: &Value) -> Result<Value, String> {
    use pi_core::messages;
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    match fn_name {
        "bashExecutionToText" => {
            let msg: messages::BashExecutionMessage =
                serde_json::from_value(input["message"].clone()).map_err(|e| e.to_string())?;
            Ok(serde_json::json!({ "text": messages::bash_execution_to_text(&msg) }))
        }
        "convertToLlm" => {
            let list = input["messages"].as_array().ok_or("missing messages array")?;
            let mut parsed = Vec::new();
            for value in list {
                parsed.push(
                    serde_json::from_value::<messages::AgentMessage>(value.clone())
                        .map_err(|e| e.to_string())?,
                );
            }
            Ok(serde_json::json!({ "messages": messages::convert_to_llm(&parsed) }))
        }
        "createBranchSummaryMessage" => {
            let summary = input["summary"].as_str().ok_or("missing summary")?;
            let from_id = input["fromId"].as_str();
            let timestamp = input["timestamp"].as_str().ok_or("missing timestamp")?;
            let msg = messages::create_branch_summary_message(summary, from_id, timestamp);
            serde_json::to_value(messages::AgentMessage::BranchSummary(msg)).map_err(|e| e.to_string())
        }
        "createCompactionSummaryMessage" => {
            let summary = input["summary"].as_str().ok_or("missing summary")?;
            let tokens_before = input["tokensBefore"].as_u64().ok_or("missing tokensBefore")?;
            let timestamp = input["timestamp"].as_str().ok_or("missing timestamp")?;
            let msg = messages::create_compaction_summary_message(summary, tokens_before, timestamp);
            serde_json::to_value(messages::AgentMessage::CompactionSummary(msg)).map_err(|e| e.to_string())
        }
        "createCustomMessage" => {
            let custom_type = input["customType"].as_str().ok_or("missing customType")?;
            let content: messages::CustomContent =
                serde_json::from_value(input["content"].clone()).map_err(|e| e.to_string())?;
            let display = input["display"].as_bool().ok_or("missing display")?;
            let details = input.get("details").filter(|v| !v.is_null()).cloned();
            let timestamp = input["timestamp"].as_str().ok_or("missing timestamp")?;
            let msg = messages::create_custom_message(custom_type, content, display, details, timestamp);
            serde_json::to_value(messages::AgentMessage::Custom(msg)).map_err(|e| e.to_string())
        }
        other => Err(format!("unknown messages fn: {}", other)),
    }
}

/// Drive pi-session `session_manager.rs` functions (mirrors
/// dist/core/session-manager.js). Relative paths in args are resolved against
/// the harness cwd, matching the TS harness's `path.resolve(cwd, ...)`.
fn run_session_manager_harness(input: &Value, cwd: &str) -> Result<Value, String> {
    use pi_core::paths::{resolve_path, PathInputOptions};
    use pi_session::session_manager as sm;
    use pi_session::session_manager::leaf_spec_from_value;

    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    let entries_arg = || -> Result<Vec<Value>, String> {
        input["entries"]
            .as_array()
            .cloned()
            .ok_or_else(|| "missing entries".to_string())
    };
    let leaf = leaf_spec_from_value(input.get("leafId"));
    match fn_name {
        "assertValidSessionId" => {
            let id = input["id"].as_str().ok_or("missing id")?;
            match sm::assert_valid_session_id(id) {
                Ok(()) => Ok(serde_json::json!({ "valid": true })),
                Err(e) => Ok(serde_json::json!({ "error": e })),
            }
        }
        "migrateSessionEntries" => {
            let mut entries = entries_arg()?;
            sm::migrate_session_entries(&mut entries);
            Ok(serde_json::json!({ "entries": entries }))
        }
        "parseSessionEntries" => {
            let content = input["content"].as_str().ok_or("missing content")?;
            Ok(serde_json::json!({ "entries": sm::parse_session_entries(content) }))
        }
        "getLatestCompactionEntry" => {
            let entries = entries_arg()?;
            Ok(serde_json::json!({ "entry": sm::get_latest_compaction_entry(&entries) }))
        }
        "sessionEntryToContextMessages" => {
            let entry = input["entry"].clone();
            if entry.is_null() {
                return Err("missing entry".to_string());
            }
            Ok(serde_json::json!({ "messages": sm::session_entry_to_context_messages(&entry) }))
        }
        "buildContextEntries" => {
            let entries = entries_arg()?;
            Ok(serde_json::json!({ "entries": sm::build_context_entries(&entries, &leaf) }))
        }
        "buildSessionProjection" => {
            let entries = entries_arg()?;
            Ok(sm::build_session_projection(&entries, &leaf).to_value())
        }
        "buildSessionContext" => {
            let entries = entries_arg()?;
            Ok(sm::build_session_context(&entries, &leaf))
        }
        "getDefaultSessionDir" => {
            let cwd_arg = input["cwd"].as_str().ok_or("missing cwd")?;
            let agent_dir = match input.get("agentDir").filter(|v| !v.is_null()) {
                Some(v) => {
                    let dir = v.as_str().ok_or("agentDir must be a string")?;
                    Some(resolve_path(dir, cwd, &PathInputOptions::default()))
                }
                None => None,
            };
            let dir = sm::get_default_session_dir(cwd_arg, agent_dir.as_deref())?;
            Ok(serde_json::json!({ "dir": dir }))
        }
        "loadEntriesFromFile" => {
            let file_path = input["filePath"].as_str().ok_or("missing filePath")?;
            let resolved = resolve_path(file_path, cwd, &PathInputOptions::default());
            let entries = sm::load_entries_from_file(&resolved)?;
            Ok(serde_json::json!({ "entries": entries }))
        }
        "findMostRecentSession" => {
            let session_dir = input["sessionDir"].as_str().ok_or("missing sessionDir")?;
            let resolved = resolve_path(session_dir, cwd, &PathInputOptions::default());
            let cwd_arg = input.get("cwd").filter(|v| !v.is_null()).and_then(Value::as_str);
            let path = sm::find_most_recent_session(&resolved, cwd_arg);
            Ok(serde_json::json!({ "path": path }))
        }
        other => Err(format!("unknown session-manager fn: {}", other)),
    }
}

/// Drive pi-session `compaction_utils.rs` (mirrors
/// dist/core/compaction/utils.js).
fn run_compaction_utils_harness(input: &Value) -> Result<Value, String> {
    use pi_session::compaction_utils as cu;

    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    match fn_name {
        "createFileOps" => Ok(serde_json::json!({ "result": cu::create_file_ops().to_value() })),
        "extractFileOpsFromMessage" => {
            let message = input["message"].clone();
            let mut file_ops = match input.get("fileOps") {
                Some(v) if !v.is_null() => cu::FileOperations::from_value(v),
                _ => cu::create_file_ops(),
            };
            cu::extract_file_ops_from_message(&message, &mut file_ops);
            Ok(serde_json::json!({ "result": file_ops.to_value() }))
        }
        "computeFileLists" => {
            let file_ops = cu::FileOperations::from_value(&input["fileOps"]);
            Ok(serde_json::json!({ "result": cu::file_lists_to_value(&cu::compute_file_lists(&file_ops)) }))
        }
        "formatFileOperations" => {
            let read_files: Vec<String> = input["readFiles"]
                .as_array()
                .map(|items| items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            let modified_files: Vec<String> = input["modifiedFiles"]
                .as_array()
                .map(|items| items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            Ok(serde_json::json!({ "result": cu::format_file_operations(&read_files, &modified_files) }))
        }
        "serializeConversation" => {
            let empty: Vec<Value> = Vec::new();
            let messages = input["messages"].as_array().unwrap_or(&empty);
            Ok(serde_json::json!({ "result": cu::serialize_conversation(messages) }))
        }
        "summarizationSystemPrompt" => {
            Ok(serde_json::json!({ "result": cu::SUMMARIZATION_SYSTEM_PROMPT }))
        }
        other => Err(format!("unknown compaction-utils fn: {}", other)),
    }
}

/// Drive pi-session `compaction.rs` (mirrors
/// dist/core/compaction/compaction.js).
fn run_compaction_harness(input: &Value) -> Result<Value, String> {
    use pi_session::compaction as c;
    use pi_session::session_manager::{build_session_projection, LeafSpec};

    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    let entries_arg = || -> Result<Vec<Value>, String> {
        input["entries"]
            .as_array()
            .cloned()
            .ok_or_else(|| "missing entries".to_string())
    };
    match fn_name {
        "defaultSettings" => Ok(serde_json::json!({ "result": c::default_compaction_settings().to_value() })),
        "calculateContextTokens" => {
            let usage = input["usage"].clone();
            Ok(serde_json::json!({ "result": pi_core::ai::js_number(c::calculate_context_tokens(&usage)) }))
        }
        "getLastAssistantUsage" => {
            let entries = entries_arg()?;
            Ok(serde_json::json!({ "result": c::get_last_assistant_usage(&entries).unwrap_or(Value::Null) }))
        }
        "estimateContextTokens" => {
            let empty: Vec<Value> = Vec::new();
            let messages = input["messages"].as_array().unwrap_or(&empty);
            Ok(serde_json::json!({ "result": c::estimate_context_tokens(messages).to_value() }))
        }
        "estimateProjectedContextTokens" => {
            let entries = entries_arg()?;
            let branch_entries = entries_arg()?;
            let projection = build_session_projection(&entries, &LeafSpec::Latest);
            Ok(serde_json::json!({ "result": c::estimate_projected_context_tokens(&projection, &branch_entries).to_value() }))
        }
        "shouldCompact" => {
            let context_tokens = input["contextTokens"].as_f64().ok_or("missing contextTokens")?;
            let context_window = input["contextWindow"].as_f64().ok_or("missing contextWindow")?;
            let settings = c::CompactionSettings::from_value(&input["settings"]);
            Ok(serde_json::json!({ "result": c::should_compact(context_tokens, context_window, &settings) }))
        }
        "estimateTokens" => {
            let message = input["message"].clone();
            Ok(serde_json::json!({ "result": pi_core::ai::js_number(c::estimate_tokens(&message)) }))
        }
        "findCutPoint" => {
            let entries = entries_arg()?;
            let start_index = input["startIndex"].as_u64().unwrap_or(0) as usize;
            let end_index = input["endIndex"]
                .as_u64()
                .map_or(entries.len(), |v| v as usize);
            let keep_recent_tokens = input["keepRecentTokens"].as_f64().ok_or("missing keepRecentTokens")?;
            Ok(serde_json::json!({ "result": c::find_cut_point(&entries, start_index, end_index, keep_recent_tokens).to_value() }))
        }
        "findTurnStartIndex" => {
            let entries = entries_arg()?;
            let entry_index = input["entryIndex"].as_u64().ok_or("missing entryIndex")? as usize;
            let start_index = input["startIndex"].as_u64().unwrap_or(0) as usize;
            Ok(serde_json::json!({ "result": c::find_turn_start_index(&entries, entry_index, start_index) }))
        }
        "getSummarizationFailure" => {
            let response = input["response"].clone();
            let label = input["label"].as_str().ok_or("missing label")?;
            let failure = c::get_summarization_failure(&response, label);
            Ok(serde_json::json!({ "result": match failure {
                Some(message) => serde_json::json!(message),
                None => Value::Null,
            } }))
        }
        "prepareCompaction" => {
            let entries = entries_arg()?;
            let settings = c::CompactionSettings::from_value(&input["settings"]);
            Ok(serde_json::json!({ "result": c::prepare_compaction(&entries, &settings).unwrap_or(Value::Null) }))
        }
        other => Err(format!("unknown compaction fn: {}", other)),
    }
}

/// Drive pi-session `nested_tool_calls.rs` (mirrors
/// dist/core/nested-tool-calls.js).
fn run_nested_calls_harness(input: &Value) -> Result<Value, String> {
    use pi_session::nested_tool_calls as nc;

    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    match fn_name {
        "limits" => Ok(serde_json::json!({ "result": nc::nested_call_limits() })),
        "recorder" => {
            let empty: Vec<Value> = Vec::new();
            let ops = input["ops"].as_array().unwrap_or(&empty);
            let mut recorder = nc::NestedCallRecorder::new();
            let mut results: Vec<Value> = Vec::new();
            for op in ops {
                match op["op"].as_str().unwrap_or("") {
                    "start" => {
                        let index = recorder.start(&op["toolCall"]);
                        results.push(serde_json::json!({ "index": match index {
                            Some(i) => serde_json::json!(i),
                            None => Value::Null,
                        } }));
                    }
                    "finish" => {
                        let index = op["index"].as_u64().map(|v| v as usize);
                        let is_error = op["isError"].as_bool().unwrap_or(false);
                        let error = op["error"].as_str().unwrap_or("");
                        recorder.finish(index, is_error, error);
                        results.push(serde_json::json!({}));
                    }
                    "addUsage" => {
                        recorder.add_usage(&op["usage"]);
                        results.push(serde_json::json!({}));
                    }
                    "totalUsage" => {
                        results.push(serde_json::json!({ "result": recorder.total_usage().unwrap_or(Value::Null) }));
                    }
                    "snapshot" => {
                        let snap = recorder.snapshot().map(|mut s| {
                            if let Some(calls) = s["calls"].as_array_mut() {
                                for call in calls {
                                    if call.get("durationMs").is_some() {
                                        call["durationMs"] = serde_json::json!(0);
                                    }
                                }
                            }
                            s
                        });
                        results.push(serde_json::json!({ "result": snap.unwrap_or(Value::Null) }));
                    }
                    other => return Err(format!("unknown nested-calls op: {}", other)),
                }
            }
            Ok(serde_json::json!({ "results": results }))
        }
        other => Err(format!("unknown nested-calls fn: {}", other)),
    }
}

/// Run `executeBashWithOperations` with local shell operations.
fn run_bash_executor_harness(input: &Value, cwd: &str) -> Result<Value, String> {
    let command = input["command"].as_str().ok_or("missing command")?;
    let ops = pi_tools::shell::create_local_bash_operations(None);
    match pi_tools::bash_executor::execute_bash_with_operations(command, cwd, &ops, None) {
        Ok(result) => serde_json::to_value(&result).map_err(|e| e.to_string()),
        Err(err) => Ok(serde_json::json!({ "error": err.message() })),
    }
}

/// Drive `OutputAccumulator` through the same sequence the bash tool uses:
/// append chunks, finish, snapshot, close the temp file, optionally read the
/// full output.
fn run_output_accumulator_harness(input: &Value) -> Result<Value, String> {
    use pi_tools::output_accumulator::{OutputAccumulator, OutputAccumulatorOptions};

    let options = OutputAccumulatorOptions {
        max_lines: input["maxLines"].as_u64(),
        max_bytes: input["maxBytes"].as_u64(),
        temp_file_prefix: input["tempFilePrefix"].as_str().map(str::to_string),
    };
    let mut acc = OutputAccumulator::new(options);
    if let Some(chunks) = input["chunks"].as_array() {
        for chunk in chunks {
            let b64 = chunk.as_str().ok_or("chunk must be a base64 string")?;
            let data = base64_decode(b64).ok_or("invalid base64")?;
            acc.append(&data).map_err(|e| e.to_string())?;
        }
    }
    let mut result = serde_json::Map::new();
    if input["finish"].as_bool().unwrap_or(true) {
        acc.finish().map_err(|e| e.to_string())?;
    }
    if input["snapshot"].as_bool().unwrap_or(true) {
        let snapshot = acc
            .snapshot(input["persistIfTruncated"].as_bool().unwrap_or(true))
            .map_err(|e| e.to_string())?;
        result.insert("snapshot".to_string(), serde_json::to_value(&snapshot).map_err(|e| e.to_string())?);
    }
    acc.close_temp_file().map_err(|e| e.to_string())?;
    if let Some(max_bytes) = input["readFullOutputMaxBytes"].as_u64() {
        let full = acc.read_full_output(max_bytes).map_err(|e| e.to_string())?;
        result.insert("full".to_string(), serde_json::to_value(&full).map_err(|e| e.to_string())?);
    }
    result.insert("lastLineBytes".to_string(), serde_json::json!(acc.get_last_line_bytes()));
    Ok(Value::Object(result))
}

fn run_pure(name: &str, cwd: &str) -> Result<Value, String> {
    let input = read_stdin_json()?;
    let value = match name {
        "normalizeForFuzzyMatch" => {
            let text = input["text"].as_str().ok_or("missing text")?;
            serde_json::json!(pi_core::normalize::normalize_for_fuzzy_match(text))
        }
        "detectLineEnding" => {
            let content = input["content"].as_str().ok_or("missing content")?;
            serde_json::json!(pi_core::edit_diff::detect_line_ending(content))
        }
        "normalizeToLF" => {
            let text = input["text"].as_str().ok_or("missing text")?;
            serde_json::json!(pi_core::edit_diff::normalize_to_lf(text))
        }
        "splitBom" => {
            let content = input["content"].as_str().ok_or("missing content")?;
            let (bom, text) = pi_core::text::split_bom(content);
            serde_json::json!({ "bom": bom, "text": text })
        }
        "formatSize" => {
            let bytes = input["bytes"].as_u64().ok_or("missing bytes")?;
            serde_json::json!(pi_core::truncate::format_size(bytes))
        }
        "truncateHead" => {
            let content = input["content"].as_str().ok_or("missing content")?;
            let opts = truncation_options(&input)?;
            serde_json::to_value(pi_core::truncate::truncate_head(content, opts))
                .map_err(|e| e.to_string())?
        }
        "truncateTail" => {
            let content = input["content"].as_str().ok_or("missing content")?;
            let opts = truncation_options(&input)?;
            serde_json::to_value(pi_core::truncate::truncate_tail(content, opts))
                .map_err(|e| e.to_string())?
        }
        "truncateMiddle" => {
            let content = input["content"].as_str().ok_or("missing content")?;
            let max_bytes = input["maxBytes"].as_u64().unwrap_or(pi_core::truncate::DEFAULT_MAX_BYTES);
            serde_json::to_value(pi_core::truncate::truncate_middle(content, max_bytes))
                .map_err(|e| e.to_string())?
        }
        "truncateLine" => {
            let line = input["line"].as_str().ok_or("missing line")?;
            let max_chars = input["maxChars"].as_u64().unwrap_or(500) as usize;
            let (text, was) = pi_core::truncate::truncate_line(line, max_chars);
            serde_json::json!({ "text": text, "wasTruncated": was })
        }
        "fuzzyFindText" => {
            let content = input["content"].as_str().ok_or("missing content")?;
            let old_text = input["oldText"].as_str().ok_or("missing oldText")?;
            let r = pi_core::edit_diff::fuzzy_find_text(content, old_text);
            serde_json::json!({
                "found": r.found,
                "index": r.index,
                "matchLength": r.match_length,
                "usedFuzzyMatch": r.used_fuzzy_match,
            })
        }
        "applyEditsToNormalizedContent" => {
            let content = input["content"].as_str().ok_or("missing content")?;
            let path = input["path"].as_str().unwrap_or("file.txt");
            let edits: Vec<pi_core::edit_diff::Edit> =
                serde_json::from_value(input["edits"].clone()).map_err(|e| e.to_string())?;
            match pi_core::edit_diff::apply_edits_to_normalized_content(content, &edits, path) {
                Ok(r) => serde_json::json!({ "baseContent": r.base_content, "newContent": r.new_content }),
                Err(e) => serde_json::json!({ "error": e }),
            }
        }
        "generateDiffString" => {
            let old = input["oldContent"].as_str().ok_or("missing oldContent")?;
            let new = input["newContent"].as_str().ok_or("missing newContent")?;
            let context = input["contextLines"].as_u64().unwrap_or(4) as usize;
            let r = pi_core::edit_diff::generate_diff_string(old, new, context);
            serde_json::json!({ "diff": r.diff, "firstChangedLine": r.first_changed_line })
        }
        "generateUnifiedPatch" => {
            let path = input["path"].as_str().unwrap_or("file.txt");
            let old = input["oldContent"].as_str().ok_or("missing oldContent")?;
            let new = input["newContent"].as_str().ok_or("missing newContent")?;
            let context = input["contextLines"].as_u64().unwrap_or(4) as usize;
            serde_json::json!(pi_core::edit_diff::generate_unified_patch(path, old, new, context))
        }
        "computeEditsDiff" => {
            let path = input["path"].as_str().ok_or("missing path")?;
            let edits: Vec<pi_core::edit_diff::Edit> =
                serde_json::from_value(input["edits"].clone()).map_err(|e| e.to_string())?;
            match pi_core::edit_diff::compute_edits_diff(path, &edits, cwd) {
                Ok(r) => serde_json::to_value(&r).map_err(|e| e.to_string())?,
                Err(e) => serde_json::json!({ "error": e }),
            }
        }
        "relativizeFindResultPath" => {
            let result_path = input["resultPath"].as_str().ok_or("missing resultPath")?;
            let search_path = input["searchPath"].as_str().ok_or("missing searchPath")?;
            serde_json::json!(pi_tools::find::relativize_find_result_path(result_path, search_path))
        }
        "expandPath" => {
            let file_path = input["path"].as_str().ok_or("missing path")?;
            serde_json::json!(pi_core::paths::expand_path(file_path))
        }
        "resolveToCwd" => {
            let file_path = input["path"].as_str().ok_or("missing path")?;
            serde_json::json!(pi_core::paths::resolve_to_cwd(file_path, cwd))
        }
        "resolveReadPath" => {
            let file_path = input["path"].as_str().ok_or("missing path")?;
            serde_json::json!(pi_core::paths::resolve_read_path(file_path, cwd))
        }
        "getCwdRelativePath" => {
            let file_path = input["path"].as_str().ok_or("missing path")?;
            let rel_cwd = input["cwd"].as_str().unwrap_or(cwd);
            serde_json::json!(pi_core::paths::get_cwd_relative_path(file_path, rel_cwd))
        }
        "detectImageMimeType" => {
            let b64 = input["base64"].as_str().ok_or("missing base64")?;
            let bytes = base64_decode(b64).ok_or("invalid base64")?;
            serde_json::json!(pi_tools::read::detect_supported_image_mime_type(&bytes))
        }
        other => return Err(format!("unknown pure function: {}", other)),
    };
    Ok(value)
}

fn truncation_options(input: &Value) -> Result<pi_core::truncate::TruncationOptions, String> {
    Ok(pi_core::truncate::TruncationOptions {
        max_lines: input.get("maxLines").and_then(|v| v.as_u64()),
        max_bytes: input.get("maxBytes").and_then(|v| v.as_u64()),
    })
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let cleaned: Vec<u8> = s.bytes().filter(|b| !b" \n\r\t".contains(b)).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 < cleaned.len() + 1 && i < cleaned.len() {
        let chunk = &cleaned[i..(i + 4).min(cleaned.len())];
        if chunk.len() < 2 {
            break;
        }
        let b0 = val(chunk[0])?;
        let b1 = val(chunk[1])?;
        let b2 = chunk.get(2).and_then(|&c| val(c));
        let b3 = chunk.get(3).and_then(|&c| val(c));
        let n = (b0 << 18) | (b1 << 12) | (b2.unwrap_or(0) << 6) | b3.unwrap_or(0);
        out.push((n >> 16) as u8);
        if b2.is_some() {
            out.push((n >> 8) as u8);
        }
        if b3.is_some() {
            out.push(n as u8);
        }
        i += 4;
    }
    Some(out)
}
