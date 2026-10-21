//! pi-cli: CLI entry point for the Rust port of the pi coding agent.
//!
//! Subcommands:
//! - `harness tool <name>`: execute a built-in tool with JSON args from
//!   stdin, print a JSON result on stdout. Used by the parity harness.
//! - `harness pure <fn>`: invoke pure pi-core functions with JSON args from
//!   stdin, print JSON result. Used by the parity harness.

use std::io::Read;

use serde_json::Value;

mod cli;
mod config;
mod migrations;
mod providers_harness;
mod config_harness;
mod resolver_harness;
mod auth_harness;
mod composer_harness;
mod runtime_harness;
mod core_harness;
mod agent_harness;
mod agent_loop_harness;
mod session_harness;
mod session_tools_harness;
mod session_prompt_harness;
mod session_run_harness;
mod session_retry_harness;
mod session_compaction_harness;
mod session_model_harness;
mod session_bash_harness;
mod session_tree_harness;
mod session_runtime_harness;
mod session_services_harness;
mod session_stats_harness;
mod extension_runner_harness;
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = run(&args);
    match result {
        Ok(value) => {
            let json = serde_json::to_string_pretty(&value).unwrap();
            // Lone-surrogate sentinels from the latex port (see
            // pi-tui/src/latex.rs) decode to `\uXXXX` escapes exactly like
            // `JSON.stringify` of a lone surrogate; adjacent sentinel pairs
            // coalesce back to the real astral char.
            println!("{}", pi_tui::latex::decode_surrogate_sentinels(&json));
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
        // Fuzz-batter batch mode: stdin is a JSON ARRAY of argument objects
        // for the SAME tool; stdout is an array of per-case results, each
        // exactly the shape the single-case `tool` mode emits (errors are
        // in-band {"error": ...}). Lets the batter framework amortize
        // process startup over thousands of cases.
        "tool-batch" => run_tool_batch(&name, &cwd),
        "pure" => run_pure(&name, &cwd),
        _ => Err(format!("unknown harness mode: {}", mode)),
    }
}

fn run_tool(name: &str, cwd: &str) -> Result<Value, String> {
    let input = read_stdin_json()?;
    run_tool_input_catching_panics(name, cwd, input)
}

/// `run_tool_input` wrapped in `catch_unwind`: a Rust panic inside a
/// harness case becomes an in-band `{error: <panic message>}` result,
/// matching the TS harness where per-case exceptions are caught and
/// reported in-band (e.g. the "Rendered line N exceeds terminal width"
/// crash that TS throws and Rust panics with). Stack overflows are
/// process aborts, NOT panics, and still crash the harness — the batter
/// reconciles that class separately.
fn run_tool_input_catching_panics(name: &str, cwd: &str, input: Value) -> Result<Value, String> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_tool_input(name, cwd, input)
    }));
    match result {
        Ok(r) => r,
        Err(payload) => {
            let mut msg = if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = payload.downcast_ref::<&str>() {
                (*s).to_string()
            } else {
                "unknown panic".to_string()
            };
            // Redact the environment-specific crash-log path so the
            // message compares equal with the TS harness output (which
            // redacts the same suffix in ts-harness.js errMsg()).
            if let Some(pos) = msg.find("Debug log written to: ") {
                msg.truncate(pos + "Debug log written to: ".len());
                msg.push_str("<redacted>");
            }
            Err(msg)
        }
    }
}

/// Batch entry used by parity/fuzz (see run-batter.mjs). Input: JSON array
/// of argument objects; output: array of per-case results.
fn run_tool_batch(name: &str, cwd: &str) -> Result<Value, String> {
    let input = read_stdin_json()?;
    let items = input
        .as_array()
        .ok_or("tool-batch input must be a JSON array of argument objects")?;
    let mut results = Vec::with_capacity(items.len());
    for item in items {
        let value = match run_tool_input_catching_panics(name, cwd, item.clone()) {
            Ok(value) => value,
            Err(message) => serde_json::json!({ "error": message }),
        };
        results.push(value);
    }
    Ok(Value::Array(results))
}

fn run_tool_input(name: &str, cwd: &str, input: Value) -> Result<Value, String> {
    // Module-level harness entries (not AgentTools): the bash executor and
    // the output accumulator, driven directly.
    match name {
        "bash-executor" => return run_bash_executor_harness(&input, cwd),
        "output-accumulator" => return run_output_accumulator_harness(&input),
        "messages" => return run_messages_harness(&input),
        "compaction-utils" => return run_compaction_utils_harness(&input),
        "file-mutation-queue" => return run_file_mutation_queue_harness(&input, std::path::Path::new(cwd)),
        "compaction" => return run_compaction_harness(&input),
        "agent" => return agent_harness::run_agent_harness(&input, std::path::Path::new(cwd)),
        "agent-queue" => {
            return agent_harness::run_agent_queue_harness(&input, std::path::Path::new(cwd))
        }
        "declare-tool-changes" => {
            return agent_harness::run_declare_tool_changes_harness(&input, std::path::Path::new(cwd))
        }
        "run-tool-call" => {
            return agent_harness::run_run_tool_call_harness(&input, std::path::Path::new(cwd))
        }
        "stream-assistant" => {
            return agent_loop_harness::run_stream_assistant_harness(&input, std::path::Path::new(cwd))
        }
        "agent-loop" => {
            return agent_loop_harness::run_agent_loop_harness(&input, std::path::Path::new(cwd))
        }
        "extension-runner" => return extension_runner_harness::run(&input),
        "agent-session-events" => return session_harness::run_agent_session_events_harness(&input),
        "agent-session-tools" => {
            return session_tools_harness::run_agent_session_tools_harness(&input)
        }
        "agent-session-prompt" => {
            return session_prompt_harness::run_agent_session_prompt_harness(&input)
        }
        "agent-session-run" => {
            return session_run_harness::run_agent_session_run_harness(&input)
        }
        "agent-session-retry" => {
            return session_retry_harness::run_agent_session_retry_harness(&input)
        }
        "agent-session-model" => {
            return session_model_harness::run_agent_session_model_harness(&input)
        }
        "agent-session-bash" => {
            return session_bash_harness::run_agent_session_bash_harness(&input)
        }
        "agent-session-tree" => {
            return session_tree_harness::run_agent_session_tree_harness(&input)
        }
        "agent-session-stats" => {
            return session_stats_harness::run_agent_session_stats_harness(&input, cwd)
        }
        "agent-session-runtime" => {
            return session_runtime_harness::run_agent_session_runtime_harness(&input, cwd)
        }
        "agent-session-services" => {
            return session_services_harness::run_agent_session_services_harness(&input, cwd)
        }
        "agent-session-compaction" => {
            return session_compaction_harness::run_agent_session_compaction_harness(&input)
        }
        "tui-utils" => return run_tui_utils_harness(&input),
        "tui-fuzzy" => return run_tui_fuzzy_harness(&input),
        "tui-word-navigation" => return run_tui_word_navigation_harness(&input),
        "tui-kill-ring" => return run_tui_kill_ring_harness(&input),
        "tui-program-status" => return run_tui_program_status_harness(&input),
        "tui-colors" => return run_tui_colors_harness(&input),
        "tui-terminal-colors" => return run_tui_terminal_colors_harness(&input),
        "tui-terminal-image" => return run_tui_terminal_image_harness(&input),
        "tui-markdown-lex" => return run_tui_markdown_lex_harness(&input),
        "tui-latex" => return run_tui_latex_harness(&input),
        "tui-markdown-render" => return run_tui_markdown_render_harness(&input),
        "tui-keys" => return run_tui_keys_harness(&input),
        "tui-stdin-buffer" => return run_tui_stdin_buffer_harness(&input),
        "tui-components" => return run_tui_components_harness(&input),
        "tui-editor" => return run_tui_editor_harness(&input),
        "tui-main" => return run_tui_screen_harness(&input, false),
        "tui-alt" => return run_tui_screen_harness(&input, true),
        "math" => return run_math_harness(&input),
        "mcp-servers" => return run_mcp_servers_harness(&input),
        "mcp-tools" => return run_mcp_tools_harness(&input),
        "tool-search" => return run_tool_search_harness(&input),
        "config-value" | "http-dispatcher" | "auth-guidance" | "models-store"
        | "runtime-credentials" | "virtual-models" => {
            return providers_harness::run(name, &input)
        }
        "provider-attribution" | "model-config" => {
            return config_harness::run(name, &input)
        }
        "model-resolver" => {
            return resolver_harness::run(name, &input)
        }
        "auth-storage" | "remote-catalog" => {
            return auth_harness::run(name, &input)
        }
        "provider-composer" => {
            return composer_harness::run(name, &input)
        }
        "model-runtime" | "model-registry" => {
            return runtime_harness::run(name, &input)
        }
        "event-bus" | "timings" | "slash-commands" | "settings-defaults"
        | "settings-schema" | "settings-manager" => {
            return core_harness::run(name, &input)
        }
        "prompt-templates" | "skills" => {
            return core_harness::run(name, &input)
        }
        "sess-manager" | "nested-tool-calls" | "pi-manifest" | "output-guard" | "defaults" => {
            return core_harness::run(name, &input)
        }
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

/// `tui-word-navigation` entry: word-granularity segmentation
/// (Intl.Segmenter) and cursor motion (word-navigation.ts) parity.
/// The Rust side runs the offline ICU 78.3 port in pi-tui; the TS side runs
/// the original module against Node's live Intl.Segmenter.
fn run_tui_word_navigation_harness(input: &Value) -> Result<Value, String> {
    // Optional atomic-segment allowlist (mirrors the isAtomicSegment option).
    let atomic: Option<std::collections::HashSet<String>> = input["atomic"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        });
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    match fn_name {
        // Batch mode for trajectory-matched differential testing: ICU's
        // language-break-engine factory is process-global, so warm-up order
        // affects results (e.g. U+FF70-led dictionary runs). `batchNav` runs
        // a sequence of segmentWords/findWordBackward/findWordForward ops in
        // ONE process so the Rust factory warms exactly like the reference.
        "batchNav" => {
            let ops = input["ops"].as_array().ok_or("missing ops array")?;
            let mut out = Vec::with_capacity(ops.len());
            for op in ops {
                out.push(run_tui_word_navigation_op(op, &atomic)?);
            }
            Ok(serde_json::json!({ "result": out }))
        }
        _ => Ok(serde_json::json!({
            "result": run_tui_word_navigation_op(input, &atomic)?
        })),
    }
}

/// One tui-word-navigation op: segmentWords (texts array) or
/// findWordBackward/findWordForward (text + cursors array).
fn run_tui_word_navigation_op(
    input: &Value,
    atomic: &Option<std::collections::HashSet<String>>,
) -> Result<Value, String> {
    use pi_tui::unicode::word_break::segment_words;
    use pi_tui::word_navigation::{self, WordNavigationOptions};
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    match fn_name {
        "segmentWords" => {
            let texts = input["texts"].as_array().ok_or("missing texts array")?;
            let mut out = Vec::with_capacity(texts.len());
            for t in texts {
                let text = t.as_str().ok_or("texts entries must be strings")?;
                let segs: Vec<Value> = segment_words(text)
                    .iter()
                    .map(|w| {
                        serde_json::json!({ "segment": w.segment, "isWordLike": w.is_word_like })
                    })
                    .collect();
                out.push(Value::Array(segs));
            }
            Ok(Value::Array(out))
        }
        "findWordBackward" | "findWordForward" => {
            let text = input["text"].as_str().ok_or("missing text")?;
            let cursors = input["cursors"].as_array().ok_or("missing cursors array")?;
            let pred = |s: &str| atomic.as_ref().is_some_and(|set| set.contains(s));
            let options = WordNavigationOptions {
                segment: None,
                is_atomic_segment: Some(&pred),
            };
            let mut results = Vec::with_capacity(cursors.len());
            for c in cursors {
                let cursor =
                    c.as_u64().ok_or("cursors entries must be non-negative integers")? as usize;
                let r = if fn_name == "findWordBackward" {
                    word_navigation::find_word_backward(text, cursor, Some(&options))
                } else {
                    word_navigation::find_word_forward(text, cursor, Some(&options))
                };
                results.push(serde_json::json!(r));
            }
            Ok(Value::Array(results))
        }
        other => Err(format!("unknown tui-word-navigation fn: {}", other)),
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

/// `tui-keys` module-level harness entry: drives the pure parser functions
/// of `packages/tui/src/keys.ts` (mirrored on the TS side by ts-harness.js).
///
/// Input: `{ cases: [{ input, kitty?, keyIds? }] }`. For each sub-case the
/// global kitty-protocol flag is set from `kitty` (this is a fresh process,
/// and the flag is reset after each sub-case so ordering can never leak),
/// then parseKey / matchesKey (per keyIds) / decodePrintableKey /
/// decodeKittyPrintable / isKeyRelease / isKeyRepeat are evaluated and
/// serialized as `{ results: [...] }` with `None` as `null`.
fn run_tui_keys_harness(input: &Value) -> Result<Value, String> {
    use pi_tui::keys;
    let cases = input["cases"].as_array().ok_or("missing cases array")?;
    let mut results = Vec::new();
    for case in cases {
        let data = case["input"].as_str().ok_or("missing input string")?;
        keys::set_kitty_protocol_active(case["kitty"].as_bool().unwrap_or(false));
        let mut entry = serde_json::json!({
            "parseKey": keys::parse_key(data),
            "decodePrintableKey": keys::decode_printable_key(data),
            "decodeKittyPrintable": keys::decode_kitty_printable(data),
            "isKeyRelease": keys::is_key_release(data),
            "isKeyRepeat": keys::is_key_repeat(data),
        });
        if let Some(ids) = case["keyIds"].as_array() {
            let mut matches = serde_json::Map::new();
            for id in ids {
                let id = id.as_str().ok_or("keyIds entries must be strings")?;
                matches.insert(id.to_string(), serde_json::json!(keys::matches_key(data, id)));
            }
            entry["matches"] = Value::Object(matches);
        }
        keys::set_kitty_protocol_active(false);
        results.push(entry);
    }
    Ok(serde_json::json!({ "results": results }))
}

/// `tui-stdin-buffer` module-level harness entry: drives
/// `packages/tui/src/stdin-buffer.ts` (mirrored on the TS side by
/// ts-harness.js).
///
/// Input: `{ cases: [{ name, kitty?, timeoutMs?, escapeTimeoutMs?,
/// script: { chunks: [string | {units:[u16]} | {bytes:[u8]}],
/// fireTimeoutAfterEachChunk?, fireTimeoutAt?: [chunk indices] } }] }`.
/// Both sides run the SAME chunk script: construct the buffer with the
/// explicit options, process chunks in order, fire the pending timeout
/// only where the script says (the TS harness patches setTimeout so the
/// real timer callback runs synchronously at exactly the same points),
/// then serialize the ordered event log (data/paste with UTF-16 unit
/// arrays), the final buffer units, and the pending-timeout kind.
fn run_tui_stdin_buffer_harness(input: &Value) -> Result<Value, String> {
    use pi_tui::keys;
    use pi_tui::stdin_buffer::{PendingTimeout, StdinBuffer, StdinBufferEvent, StdinBufferOptions};
    let cases = input["cases"].as_array().ok_or("missing cases array")?;
    let mut results = Vec::new();
    for case in cases {
        keys::set_kitty_protocol_active(case["kitty"].as_bool().unwrap_or(false));
        let options = StdinBufferOptions {
            timeout_ms: case["timeoutMs"].as_u64().unwrap_or(50),
            escape_timeout_ms: case["escapeTimeoutMs"].as_u64().unwrap_or(10),
        };
        let script = &case["script"];
        let chunks = script["chunks"].as_array().ok_or("missing chunks array")?;
        let fire_after_each = script["fireTimeoutAfterEachChunk"].as_bool().unwrap_or(false);
        let fire_at: Vec<usize> = script["fireTimeoutAt"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_u64().map(|u| u as usize))
                    .collect()
            })
            .unwrap_or_default();
        let mut buffer = StdinBuffer::new(options);
        for (i, chunk) in chunks.iter().enumerate() {
            match chunk {
                Value::String(s) => buffer.process(s),
                Value::Object(o) if o.get("units").map(Value::is_array).unwrap_or(false) => {
                    let units: Vec<u16> = o["units"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(|v| v.as_u64().and_then(|u| u16::try_from(u).ok()))
                        .collect();
                    buffer.process_units(&units);
                }
                Value::Object(o) if o.get("bytes").map(Value::is_array).unwrap_or(false) => {
                    let bytes: Vec<u8> = o["bytes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(|v| v.as_u64().and_then(|u| u8::try_from(u).ok()))
                        .collect();
                    buffer.process_bytes(&bytes);
                }
                _ => return Err("invalid chunk: expected string, {units}, or {bytes}".to_string()),
            }
            if fire_after_each || fire_at.contains(&i) {
                buffer.fire_timeout();
            }
        }
        let events: Vec<Value> = buffer
            .take_events()
            .into_iter()
            .map(|event| match event {
                StdinBufferEvent::Data(units) => {
                    serde_json::json!({ "type": "data", "units": units })
                }
                StdinBufferEvent::Paste(units) => {
                    serde_json::json!({ "type": "paste", "units": units })
                }
            })
            .collect();
        let pending = match buffer.pending_timeout() {
            Some(PendingTimeout::Escape) => serde_json::json!("escape"),
            Some(PendingTimeout::Sequence) => serde_json::json!("sequence"),
            None => Value::Null,
        };
        results.push(serde_json::json!({
            "events": events,
            "buffer": buffer.buffer().to_vec(),
            "pendingTimeout": pending,
        }));
        buffer.destroy();
        keys::set_kitty_protocol_active(false);
    }
    Ok(serde_json::json!({ "results": results }))
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

/// Module harness for `pi_core::mcp_servers` (TS: `core/mcp-servers.ts`).
fn run_mcp_servers_harness(input: &Value) -> Result<Value, String> {
    use pi_core::mcp_servers as ms;
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    let str_list = |key: &str| -> Result<Vec<String>, String> {
        input[key]
            .as_array()
            .ok_or_else(|| format!("missing {}", key))?
            .iter()
            .map(|v| v.as_str().map(str::to_string).ok_or_else(|| format!("non-string in {}", key)))
            .collect()
    };
    match fn_name {
        "createToolNameMatcher" => {
            let entries = str_list("entries")?;
            let matcher = ms::create_tool_name_matcher(&entries);
            let names = str_list("names")?;
            Ok(serde_json::json!({
                "matches": names.iter().map(|n| matcher.matches(n)).collect::<Vec<bool>>(),
            }))
        }
        "isMcpToolName" => {
            let names = str_list("names")?;
            Ok(serde_json::json!({
                "results": names.iter().map(|n| ms::is_mcp_tool_name(n)).collect::<Vec<bool>>(),
            }))
        }
        "mcpNamespace" => {
            let servers = str_list("servers")?;
            Ok(serde_json::json!({
                "namespaces": servers.iter().map(|s| ms::mcp_namespace(s)).collect::<Vec<String>>(),
            }))
        }
        "validateMcpServerConfig" => {
            let name = input["name"].as_str().ok_or("missing name")?;
            let raw = input.get("raw").cloned().unwrap_or(Value::Null);
            match ms::validate_mcp_server_config(name, &raw) {
                Ok(config) => Ok(serde_json::json!({
                    "result": serde_json::to_value(&config).map_err(|e| e.to_string())?,
                })),
                Err(error) => Ok(serde_json::json!({ "result": { "error": error } })),
            }
        }
        "getMcpToolExposure" => {
            let name = input["name"].as_str().ok_or("missing name")?;
            let raw = input.get("raw").cloned().unwrap_or(Value::Null);
            let config = ms::validate_mcp_server_config(name, &raw)?;
            let tool_names = str_list("toolNames")?;
            let exposures = tool_names
                .iter()
                .map(|n| serde_json::to_value(ms::get_mcp_tool_exposure(&config, n)))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            Ok(serde_json::json!({ "exposures": exposures }))
        }
        "isLoopbackRedirectUri" => {
            let values = str_list("values")?;
            Ok(serde_json::json!({
                "results": values.iter().map(|v| ms::is_loopback_redirect_uri(v)).collect::<Vec<bool>>(),
            }))
        }
        "parseUrl" => {
            let inputs = str_list("inputs")?;
            let results = inputs
                .iter()
                .map(|input| match ms::parse_url(input) {
                    Some(url) => serde_json::json!({
                        "protocol": url.protocol,
                        "hostname": url.hostname,
                        "port": url.port,
                        "pathname": url.pathname,
                        "search": url.search,
                        "hash": url.hash,
                    }),
                    None => Value::Null,
                })
                .collect::<Vec<_>>();
            Ok(serde_json::json!({ "results": results }))
        }
        _ => Err(format!("unknown mcp-servers fn: {fn_name}")),
    }
}

/// Module harness for `pi_core::mcp_tools` (TS: `extensions/mcp/tools.ts`).
/// The output saver is a deterministic fake on both sides
/// (`async (_, ext) => "/tmp/pi-mcp-out" + ext`), so truncated-output and
/// binary-resource cases compare without temp-file normalization.
fn run_mcp_tools_harness(input: &Value) -> Result<Value, String> {
    use pi_core::mcp_tools as mt;
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    let fake_saver = |_data: mt::OutputData, extension: &str| -> Result<String, String> {
        Ok(format!("/tmp/pi-mcp-out{}", extension))
    };
    match fn_name {
        "createMcpToolName" => {
            let calls = input["calls"].as_array().ok_or("missing calls")?;
            let mut results = Vec::with_capacity(calls.len());
            for call in calls {
                let server = call["server"].as_str().ok_or("missing server")?;
                let tool = call["tool"].as_str().ok_or("missing tool")?;
                let taken: Vec<String> = call["taken"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                results.push(mt::create_mcp_tool_name(server, tool, |name| taken.iter().any(|t| t == name)));
            }
            Ok(serde_json::json!({ "results": results }))
        }
        "createMcpResultSchema" => {
            let schema = mt::create_mcp_result_schema(input.get("structuredContentSchema"));
            Ok(serde_json::json!({ "schema": schema }))
        }
        "toToolExposure" => {
            let exposures: Vec<String> = input["exposures"]
                .as_array()
                .ok_or("missing exposures")?
                .iter()
                .map(|v| v.as_str().map(str::to_string).ok_or("non-string exposure"))
                .collect::<Result<_, _>>()?;
            let results = exposures
                .iter()
                .map(|e| {
                    let exposure = pi_core::mcp_servers::McpExposure::parse(e)
                        .ok_or_else(|| format!("unknown exposure: {}", e))?;
                    serde_json::to_value(mt::to_tool_exposure(exposure)).map_err(|err| err.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(serde_json::json!({ "results": results }))
        }
        "toModelContent" => {
            let server = input["server"].as_str().ok_or("missing server")?;
            let blocks = parse_content_blocks(input)?;
            let options = mt::ConvertMcpResultOptions {
                save_output: &fake_saver,
                readable_resources: input["readableResources"].as_bool().unwrap_or(false),
            };
            let content = mt::to_model_content(server, &blocks, &options);
            let content = content
                .iter()
                .map(|c| serde_json::to_value(c))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            Ok(serde_json::json!({ "content": content }))
        }
        "convertMcpResult" => {
            let server = input["server"].as_str().ok_or("missing server")?;
            let tool = input["tool"].as_str().ok_or("missing tool")?;
            let result = parse_call_tool_result(input)?;
            let options = mt::ConvertMcpResultOptions {
                save_output: &fake_saver,
                readable_resources: input["readableResources"].as_bool().unwrap_or(false),
            };
            let converted = mt::convert_mcp_result(server, tool, &result, &options)?;
            Ok(serde_json::to_value(&converted).map_err(|e| e.to_string())?)
        }
        "limitMcpContent" => {
            let content = parse_llm_content(input)?;
            let limited = mt::limit_mcp_content(&content, &fake_saver);
            let content = limited
                .content
                .iter()
                .map(|c| serde_json::to_value(c))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            let mut out = serde_json::json!({ "content": content });
            if let Some(path) = limited.full_output_path {
                out["fullOutputPath"] = Value::String(path);
            }
            Ok(out)
        }
        _ => Err(format!("unknown mcp-tools fn: {fn_name}")),
    }
}

fn parse_content_blocks(input: &Value) -> Result<Vec<pi_core::mcp_tools::ContentBlock>, String> {
    let blocks = input["blocks"].as_array().ok_or("missing blocks")?;
    blocks
        .iter()
        .map(|b| {
            b.as_object()
                .cloned()
                .map(pi_core::mcp_tools::ContentBlock::from)
                .ok_or_else(|| "non-object block".to_string())
        })
        .collect()
}

fn parse_call_tool_result(input: &Value) -> Result<pi_core::mcp_tools::CallToolResult, String> {
    input["result"]
        .as_object()
        .cloned()
        .map(pi_core::mcp_tools::CallToolResult::from)
        .ok_or_else(|| "missing result object".to_string())
}

fn parse_llm_content(input: &Value) -> Result<Vec<pi_core::mcp_tools::LlmContent>, String> {
    let content = input["content"].as_array().ok_or("missing content")?;
    content
        .iter()
        .map(|c| serde_json::from_value::<pi_core::messages::TextOrImage>(c.clone()).map_err(|e| e.to_string()))
        .collect()
}

/// Module harness for `pi_core::tool_search`
/// (TS: `extensions/tool-search/tool.ts`).
fn run_tool_search_harness(input: &Value) -> Result<Value, String> {
    use pi_core::tool_search as tsm;
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    match fn_name {
        "tokenize" => {
            let texts: Vec<String> = input["texts"]
                .as_array()
                .ok_or("missing texts")?
                .iter()
                .map(|v| v.as_str().map(str::to_string).ok_or("non-string text"))
                .collect::<Result<_, _>>()?;
            Ok(serde_json::json!({
                "tokens": texts.iter().map(|t| tsm::tokenize(t)).collect::<Vec<_>>(),
            }))
        }
        "createToolSearchDocument" => {
            let tool = input["tool"].as_object().ok_or("missing tool")?;
            let tool = tsm::ToolInfo {
                name: tool.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                description: tool
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                parameters: tool.get("parameters").cloned().unwrap_or(Value::Null),
            };
            let namespace = input.get("namespace").map(|ns| tsm::ToolNamespace {
                name: ns.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                description: ns.get("description").and_then(|v| v.as_str()).map(str::to_string),
                instructions: ns.get("instructions").and_then(|v| v.as_str()).map(str::to_string),
            });
            let document = tsm::create_tool_search_document(&tool, namespace.as_ref());
            Ok(serde_json::json!({ "name": document.name, "text": document.text }))
        }
        "rank" => {
            let query = input["query"].as_str().ok_or("missing query")?;
            let documents: Vec<tsm::ToolSearchDocument> = input["documents"]
                .as_array()
                .ok_or("missing documents")?
                .iter()
                .map(|d| {
                    tsm::ToolSearchDocument {
                        name: d.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                        text: d.get("text").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                    }
                })
                .collect();
            let limit = input["limit"].as_u64().ok_or("missing limit")? as usize;
            let ranker = match (input.get("k1"), input.get("b")) {
                (Some(k1), Some(b)) => tsm::Bm25Ranker::with_params(k1.as_f64().unwrap(), b.as_f64().unwrap()),
                _ => tsm::Bm25Ranker::new(),
            };
            let matches = ranker.rank(query, &documents, limit);
            Ok(serde_json::json!({
                "matches": matches
                    .iter()
                    .map(|m| serde_json::json!({ "name": m.name, "score": m.score }))
                    .collect::<Vec<_>>(),
            }))
        }
        _ => Err(format!("unknown tool-search fn: {fn_name}")),
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
            // V8's Math.log is its fdlibm port, which differs from the
            // system libm's ln for ~0.7% of inputs by 1 ulp.
            ("log", [x]) => pi_core::js_math::js_math_log(*x),
            ("atan", [x]) => x.atan(),
            _ => return Err(format!("unknown math fn: {}", fn_name)),
        };
        results.push(format!("{:016x}", value.to_bits()));
    }
    Ok(serde_json::json!({ "results": results }))
}
fn run_tui_markdown_lex_harness(input: &Value) -> Result<Value, String> {
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    match fn_name {
        "lex" => {
            let source = input["source"]
                .as_str()
                .ok_or("missing source")?
                .to_string();
            let tokens = pi_tui::marked::lex_markdown(&source);
            Ok(serde_json::json!({
                "result": pi_tui::marked::tokens_to_json(&tokens),
            }))
        }
        _ => Err(format!("unknown tui-markdown-lex fn: {fn_name}")),
    }
}

fn run_tui_latex_harness(input: &Value) -> Result<Value, String> {
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    match fn_name {
        "render" => {
            let source = input["source"]
                .as_str()
                .ok_or("missing source")?
                .to_string();
            let display = input["display"].as_bool().unwrap_or(false);
            let result = pi_tui::latex::render_latex(
                &source,
                &pi_tui::latex::RenderLatexOptions {
                    display: Some(display),
                },
            );
            // null (not absent) so both sides serialize identically.
            Ok(serde_json::json!({ "result": result }))
        }
        _ => Err(format!("unknown tui-latex fn: {fn_name}")),
    }
}

fn run_tui_markdown_render_harness(input: &Value) -> Result<Value, String> {
    use pi_tui::components::markdown::{Markdown, MarkdownOptions, MarkdownTheme};

    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    match fn_name {
        "render" => {
            let source = input["source"]
                .as_str()
                .ok_or("missing source")?
                .to_string();
            let padding_x = input["paddingX"].as_u64().unwrap_or(0) as usize;
            let padding_y = input["paddingY"].as_u64().unwrap_or(0) as usize;
            let width = input["width"].as_u64().unwrap_or(40) as usize;
            // Deterministic marker theme shared 1:1 with the TS harness.
            let theme = MarkdownTheme {
                heading: Box::new(|t: &str| format!("#<{t}>")),
                link: Box::new(|t: &str| format!("L<{t}>")),
                link_url: Box::new(|t: &str| format!("U<{t}>")),
                code: Box::new(|t: &str| format!("C<{t}>")),
                code_block: Box::new(|t: &str| format!("CB<{t}>")),
                code_block_border: Box::new(|t: &str| format!("B<{t}>")),
                quote: Box::new(|t: &str| format!("Q<{t}>")),
                quote_border: Box::new(|_t: &str| "|".to_string()),
                hr: Box::new(|t: &str| format!("-{t}-")),
                list_bullet: Box::new(|t: &str| format!("*{t}")),
                bold: Box::new(|t: &str| format!("**{t}**")),
                italic: Box::new(|t: &str| format!("_{t}_")),
                strikethrough: Box::new(|t: &str| format!("~{t}~")),
                underline: Box::new(|t: &str| format!("_<{t}>_")),
                highlight_code: None,
                code_block_indent: None,
            };
            let mut options = MarkdownOptions::default();
            if let Some(o) = input.get("options") {
                options.preserve_ordered_list_markers =
                    o["preserveOrderedListMarkers"].as_bool();
                options.preserve_backslash_escapes = o["preserveBackslashEscapes"].as_bool();
                options.render_latex = o["renderLatex"].as_bool();
            }
            let mut component =
                Markdown::new(source, padding_x, padding_y, theme, None, Some(options));
            let lines = pi_tui::component::Component::render(&mut component, width);
            Ok(serde_json::json!({ "result": lines }))
        }
        _ => Err(format!("unknown tui-markdown-render fn: {fn_name}")),
    }
}

fn run_tui_terminal_image_harness(input: &Value) -> Result<Value, String> {
    use pi_tui::terminal_image::{
        self, calculate_image_cell_size, calculate_image_rows, encode_iterm2, encode_kitty,
        get_gif_dimensions, get_image_dimensions, get_png_dimensions, get_webp_dimensions,
        hyperlink, image_fallback, render_image, CellDimensions, ImageProtocol,
        Iterm2EncodeOptions, KittyEncodeOptions, TerminalCapabilities,
    };
    let fn_name = input["fn"].as_str().ok_or("missing fn")?;
    let text = |key: &str| -> Result<&str, String> {
        input[key].as_str().ok_or_else(|| format!("missing {key}"))
    };

    // `args.setup`: capabilities / cell dimensions applied before the call.
    let mut setup_applied = false;
    if let Some(setup) = input.get("setup") {
        setup_applied = true;
        if let Some(caps) = setup.get("capabilities") {
            let images = match caps["images"].as_str() {
                Some("kitty") => Some(ImageProtocol::Kitty),
                Some("iterm2") => Some(ImageProtocol::Iterm2),
                _ => None,
            };
            terminal_image::set_capabilities(TerminalCapabilities {
                images,
                true_color: caps["trueColor"].as_bool().unwrap_or(false),
                hyperlinks: caps["hyperlinks"].as_bool().unwrap_or(false),
            });
        }
        if let Some(dims) = setup.get("cellDimensions") {
            terminal_image::set_cell_dimensions(
                dims["widthPx"].as_u64().unwrap_or(9) as u32,
                dims["heightPx"].as_u64().unwrap_or(18) as u32,
            );
        }
    }

    let result = (|| -> Result<Value, String> {
        match fn_name {
            "encodeKitty" => {
                let options = input.get("options");
                let opts = KittyEncodeOptions {
                    columns: options.and_then(|o| o.get("columns")).and_then(|v| v.as_i64()),
                    rows: options.and_then(|o| o.get("rows")).and_then(|v| v.as_i64()),
                    image_id: options.and_then(|o| o.get("imageId")).and_then(|v| v.as_i64()),
                    move_cursor: options
                        .and_then(|o| o.get("moveCursor"))
                        .and_then(|v| v.as_bool()),
                };
                Ok(serde_json::json!({ "result": encode_kitty(text("base64Data")?, &opts) }))
            }
            "encodeITerm2" => {
                let options = input.get("options");
                // TS width/height are number-or-string; stringify JSON values.
                let dim = |key: &str| -> Option<String> {
                    options
                        .and_then(|o| o.get(key))
                        .filter(|v| !v.is_null())
                        .map(|v| match v {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        })
                };
                let opts = Iterm2EncodeOptions {
                    width: dim("width"),
                    height: dim("height"),
                    name: options
                        .and_then(|o| o.get("name"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    preserve_aspect_ratio: options
                        .and_then(|o| o.get("preserveAspectRatio"))
                        .and_then(|v| v.as_bool()),
                    inline: options
                        .and_then(|o| o.get("inline"))
                        .and_then(|v| v.as_bool()),
                };
                Ok(serde_json::json!({ "result": encode_iterm2(text("base64Data")?, &opts) }))
            }
            "calculateImageCellSize" => {
                let image_dimensions = json_dims(&input["imageDimensions"])?;
                let max_width = input["maxWidthCells"].as_i64().ok_or("missing maxWidthCells")?;
                let max_height = input["maxHeightCells"].as_i64();
                let cell_dimensions = input
                    .get("cellDimensions")
                    .map(json_dims)
                    .transpose()?
                    .map(|d| CellDimensions { width_px: d.width_px as u32, height_px: d.height_px as u32 })
                    .unwrap_or(CellDimensions { width_px: 9, height_px: 18 });
                let optimize = input["optimizeAspectRatio"].as_bool().unwrap_or(false);
                let size = calculate_image_cell_size(
                    image_dimensions,
                    max_width,
                    max_height,
                    cell_dimensions,
                    optimize,
                );
                Ok(serde_json::json!({ "result": { "columns": size.columns, "rows": size.rows } }))
            }
            "calculateImageRows" => {
                let image_dimensions = json_dims(&input["imageDimensions"])?;
                let target_width = input["targetWidthCells"].as_i64().ok_or("missing targetWidthCells")?;
                let cell_dimensions = input
                    .get("cellDimensions")
                    .map(json_dims)
                    .transpose()?
                    .map(|d| CellDimensions { width_px: d.width_px as u32, height_px: d.height_px as u32 })
                    .unwrap_or(CellDimensions { width_px: 9, height_px: 18 });
                Ok(serde_json::json!({ "result": calculate_image_rows(image_dimensions, target_width, cell_dimensions) }))
            }
            "getPngDimensions" => Ok(serde_json::json!({
                "result": json_opt_dims(get_png_dimensions(text("base64Data")?))
            })),
            "getGifDimensions" => Ok(serde_json::json!({
                "result": json_opt_dims(get_gif_dimensions(text("base64Data")?))
            })),
            "getWebpDimensions" => Ok(serde_json::json!({
                "result": json_opt_dims(get_webp_dimensions(text("base64Data")?))
            })),
            "getImageDimensions" => Ok(serde_json::json!({
                "result": json_opt_dims(get_image_dimensions(text("base64Data")?, text("mimeType")?))
            })),
            "isImageLine" => Ok(serde_json::json!({ "result": terminal_image::is_image_line(text("line")?) })),
            "renderImage" => {
                let image_dimensions = json_dims(&input["imageDimensions"])?;
                let options = input.get("options");
                let opts = json_render_options(options);
                let result = render_image(text("base64Data")?, image_dimensions, &opts);
                Ok(serde_json::json!({ "result": json_opt_rendered(result) }))
            }
            "renderImageAndCrop" => {
                let image_dimensions = json_dims(&input["imageDimensions"])?;
                let options = input.get("options");
                let opts = json_render_options(options);
                let hidden_rows = input["hiddenRows"].as_i64().unwrap_or(0);
                let visible_rows = input["visibleRows"].as_i64().unwrap_or(0);
                let result = match render_image(text("base64Data")?, image_dimensions, &opts) {
                    Some(result) => result,
                    None => return Ok(serde_json::json!({ "result": null })),
                };
                let cropped =
                    terminal_image::crop_kitty_image_line(&result.sequence, hidden_rows, visible_rows);
                let placement = terminal_image::get_kitty_image_placement(&cropped);
                let mut json = json_rendered_value(&result);
                json["cropped"] = serde_json::json!(cropped);
                json["placement"] = match placement {
                    Some(placement) => serde_json::json!({
                        "imageId": placement.image_id,
                        "transmissionGeneration": placement.transmission_generation,
                        "transmissionBytes": placement.transmission_bytes,
                        "estimatedDecodedBytes": placement.estimated_decoded_bytes,
                        "rows": placement.rows,
                        "sequence": placement.sequence,
                        "replacementLine": placement.replacement_line,
                    }),
                    None => serde_json::json!(null),
                };
                Ok(serde_json::json!({ "result": json }))
            }
            "hyperlink" => Ok(serde_json::json!({ "result": hyperlink(text("text")?, text("url")?) })),
            "imageFallback" => {
                let dimensions = input
                    .get("dimensions")
                    .filter(|v| !v.is_null())
                    .map(json_dims)
                    .transpose()?;
                let filename = input["filename"].as_str().filter(|s| !s.is_empty());
                Ok(serde_json::json!({
                    "result": image_fallback(text("mimeType")?, dimensions, filename)
                }))
            }
            _ => Err(format!("unknown tui-terminal-image fn: {fn_name}")),
        }
    })();

    if setup_applied {
        terminal_image::reset_capabilities_cache();
        terminal_image::set_cell_dimensions(9, 18);
    }
    result
}

/// Parse `ImageDimensions` from `{ widthPx, heightPx }` JSON.
fn json_dims(value: &Value) -> Result<pi_tui::terminal_image::ImageDimensions, String> {
    Ok(pi_tui::terminal_image::ImageDimensions {
        width_px: value["widthPx"].as_i64().ok_or("missing widthPx")?,
        height_px: value["heightPx"].as_i64().ok_or("missing heightPx")?,
    })
}

/// Serialize `Option<ImageDimensions>` — `null` when absent.
fn json_opt_dims(dims: Option<pi_tui::terminal_image::ImageDimensions>) -> Value {
    match dims {
        Some(d) => serde_json::json!({ "widthPx": d.width_px, "heightPx": d.height_px }),
        None => serde_json::json!(null),
    }
}

/// Parse `ImageRenderOptions` from harness JSON.
fn json_render_options(options: Option<&Value>) -> pi_tui::terminal_image::ImageRenderOptions {
    let get = |key: &str| options.and_then(|o| o.get(key)).filter(|v| !v.is_null());
    pi_tui::terminal_image::ImageRenderOptions {
        max_width_cells: get("maxWidthCells").and_then(|v| v.as_i64()),
        max_height_cells: get("maxHeightCells").and_then(|v| v.as_i64()),
        preserve_aspect_ratio: get("preserveAspectRatio").and_then(|v| v.as_bool()),
        image_id: get("imageId").and_then(|v| v.as_i64()),
        move_cursor: get("moveCursor").and_then(|v| v.as_bool()),
    }
}

/// Serialize `render_image`'s result — `imageId` is dropped when `None`
/// (TS `undefined` keys vanish under JSON.stringify).
fn json_rendered_value(result: &pi_tui::terminal_image::RenderedImage) -> Value {
    let mut json = serde_json::json!({
        "sequence": result.sequence,
        "columns": result.columns,
        "rows": result.rows,
    });
    if let Some(image_id) = result.image_id {
        json["imageId"] = serde_json::json!(image_id);
    }
    json
}

/// Serialize `Option<RenderedImage>` — `null` when the protocol is absent.
fn json_opt_rendered(result: Option<pi_tui::terminal_image::RenderedImage>) -> Value {
    match result {
        Some(result) => json_rendered_value(&result),
        None => serde_json::json!(null),
    }
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
            let display = input.get("display").and_then(Value::as_bool);
            let details = input.get("details").filter(|v| !v.is_null()).cloned();
            let timestamp = input["timestamp"].as_str().ok_or("missing timestamp")?;
            let msg = messages::create_custom_message(custom_type, content, display, details, timestamp);
            serde_json::to_value(messages::AgentMessage::Custom(msg)).map_err(|e| e.to_string())
        }
        other => Err(format!("unknown messages fn: {}", other)),
    }
}

/// Drive pi-session `compaction_utils.rs` (mirrors
/// dist/core/compaction/utils.js).
fn run_compaction_utils_harness(input: &Value) -> Result<Value, String> {
    use pi_core::compaction_utils as cu;

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
    use pi_core::compaction as c;
    use pi_core::session_manager::{build_session_projection, LeafId};

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
            let projection = build_session_projection(&entries, LeafId::Unset, None);
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

/// `tui-components` module-level harness entry: differential render-line
/// testing of the TUI components (mirrored on the TS side by ts-harness.js).
///
/// Input: `{ cases: [{ name, component: <spec tree>, widths?: [n],
/// script?: [{ inputs?: [str], ticks?: n, setFilter?: str,
/// scroll?: [{updateLayout?, scrollTo?, scrollBy?, scrollToStart?,
/// scrollToEnd?, disableFollow?}], mouse?: {...}, render?: bool,
/// width?: n }] }] }`.
///
/// Each sub-case builds the SAME component tree on both sides (specs are
/// JSON; function-valued params are named presets mapped identically by
/// both harnesses — style presets "identity"/"bracket", callback preset
/// "record" whose events land in the compared event log), renders it at
/// the given widths, optionally runs a scripted input/tick/scroll/mouse
/// sequence with re-renders, and outputs the rendered lines, the recorder
/// event log, and component state. Loader animation ticks call the clock
/// seam (`Loader::tick`) exactly as often as the TS harness invokes the
/// captured setInterval callback.
fn run_tui_components_harness(input: &Value) -> Result<Value, String> {
    use std::cell::RefCell;
    use std::rc::Rc;

    use pi_tui::component::{
        dispatch_mouse_event, ComponentRef, Focusable, TuiMouseButton, TuiMouseEvent,
        TuiMouseEventType,
    };
    use pi_tui::components::box_component::TuiBox;
    use pi_tui::components::cancellable_loader::CancellableLoader;
    use pi_tui::components::h_stack::HStack;
    use pi_tui::components::input::{Input, InputOptions};
    use pi_tui::components::loader::{register_loader, Loader, LoaderIndicatorOptions};
    use pi_tui::components::mouse_region::MouseRegion;
    use pi_tui::components::scroll_view::{
        ScrollView, ScrollViewFollow, ScrollViewOptions, ScrollViewScrollToOptions,
    };
    use pi_tui::components::select_list::{
        SelectItem, SelectList, SelectListLayoutOptions, SelectListTheme,
    };
    use pi_tui::components::settings_list::{
        SettingItem, SettingsList, SettingsListOptions, SettingsListTheme,
    };
    use pi_tui::components::spacer::Spacer;
    use pi_tui::components::stack::{StackChild, StackEntryOptions, StackOptions};
    use pi_tui::components::text::{CustomBgFn, Text};
    use pi_tui::components::truncated_text::TruncatedText;
    use pi_tui::components::v_stack::VStack;
    use pi_tui::layout_node::{Basis, LayoutViewport, StackAlign};

    type Log = Rc<RefCell<Vec<Value>>>;

    /// A built sub-case component: the `ComponentRef` handle plus the typed
    /// handles the script steps need (input/scroll/tick/state accessors).
    enum Built {
        Plain(ComponentRef),
        Input(Rc<RefCell<Input>>),
        Select(Rc<RefCell<SelectList>>),
        Scroll(Rc<RefCell<ScrollView>>),
        Loader(Rc<RefCell<Loader>>),
        CancelLoader(Rc<RefCell<CancellableLoader>>),
    }

    impl Built {
        fn comp(&self) -> ComponentRef {
            match self {
                Built::Plain(c) => Rc::clone(c),
                Built::Input(c) => {
                    let erased: ComponentRef = c.clone();
                    erased
                }
                Built::Select(c) => {
                    let erased: ComponentRef = c.clone();
                    erased
                }
                Built::Scroll(c) => {
                    let erased: ComponentRef = c.clone();
                    erased
                }
                Built::Loader(c) => {
                    let erased: ComponentRef = c.clone();
                    erased
                }
                Built::CancelLoader(c) => {
                    let erased: ComponentRef = c.clone();
                    erased
                }
            }
        }
    }

    fn bg_preset(preset: Option<&str>) -> Option<CustomBgFn> {
        match preset {
            Some("identity") => Some(Box::new(|t: &str| t.to_string())),
            Some("bracket") => Some(Box::new(|t: &str| format!("[{}]", t))),
            _ => None,
        }
    }

    fn style_preset() -> Box<dyn Fn(&str) -> String> {
        Box::new(|t: &str| t.to_string())
    }

    fn mouse_event_json(ev: &TuiMouseEvent) -> Value {
        serde_json::json!({
            "kind": "mouse",
            "type": ev.event_type.as_str(),
            "button": ev.button.as_str(),
            "x": ev.x,
            "y": ev.y,
            "width": ev.width,
            "height": ev.height,
            "shift": ev.shift,
            "alt": ev.alt,
            "ctrl": ev.ctrl,
            "wheelDelta": ev.wheel_delta,
            "clickCount": ev.click_count,
        })
    }

    fn item_event_json(kind: &str, item: &SelectItem) -> Value {
        serde_json::json!({
            "kind": kind,
            "value": item.value,
            "label": item.label,
            "description": item.description,
        })
    }

    fn build(spec: &Value, log: &Log) -> Result<Built, String> {
        let type_ = spec["type"].as_str().ok_or("missing component type")?;
        let num = |key: &str, default: u64| -> u64 {
            spec[key]
                .as_f64()
                .map(|f| f.max(0.0) as u64)
                .unwrap_or(default)
        };
        match type_ {
            "text" => Ok(Built::Plain(Rc::new(RefCell::new(Text::new(
                spec["text"].as_str().unwrap_or(""),
                num("paddingX", 1) as usize,
                num("paddingY", 1) as usize,
                bg_preset(spec["bg"].as_str()),
            ))))),
            "truncated-text" => Ok(Built::Plain(Rc::new(RefCell::new(
                TruncatedText::new(
                    spec["text"].as_str().unwrap_or(""),
                    num("paddingX", 0) as usize,
                    num("paddingY", 0) as usize,
                ),
            )))),
            "spacer" => Ok(Built::Plain(Rc::new(RefCell::new(Spacer::new(
                num("lines", 1) as usize,
            ))))),
            "box" => {
                let mut box_ = TuiBox::new(
                    num("paddingX", 1) as usize,
                    num("paddingY", 1) as usize,
                    bg_preset(spec["bg"].as_str()),
                );
                let empty: Vec<Value> = Vec::new();
                for child in spec["children"].as_array().unwrap_or(&empty) {
                    box_.add_child(build(child, log)?.comp());
                }
                Ok(Built::Plain(Rc::new(RefCell::new(box_))))
            }
            "h-stack" | "v-stack" => {
                let mut children: Vec<StackChild> = Vec::new();
                let empty: Vec<Value> = Vec::new();
                for entry in spec["entries"].as_array().unwrap_or(&empty) {
                    let component = build(&entry["component"], log)?.comp();
                    let mut options = StackEntryOptions::default();
                    if entry["basis"].as_str() == Some("auto") {
                        options.basis = Some(Basis::Auto);
                    } else if let Some(b) = entry["basis"].as_f64() {
                        options.basis = Some(Basis::Points(b));
                    }
                    if let Some(g) = entry["grow"].as_f64() {
                        options.grow = Some(g);
                    }
                    if let Some(s) = entry["shrink"].as_f64() {
                        options.shrink = Some(s);
                    }
                    if let Some(m) = entry["minSize"].as_f64() {
                        options.min_size = Some(m);
                    }
                    if let Some(m) = entry["maxSize"].as_f64() {
                        options.max_size = Some(m);
                    }
                    match entry["visible"].as_str() {
                        Some("always") => {
                            options.visible = Some(Rc::new(|_vp: &LayoutViewport| true));
                        }
                        // TS maps ANY other present value to never-visible
                        // (`entry.visible === "always" ? () => true : () => false`).
                        _ if entry.get("visible").is_some() => {
                            options.visible = Some(Rc::new(|_vp: &LayoutViewport| false));
                        }
                        _ => {}
                    }
                    children.push(StackChild::Entry { component, options });
                }
                let mut stack_options = StackOptions::default();
                if let Some(g) = spec["gap"].as_f64() {
                    stack_options.gap = Some(g);
                }
                if let Some(a) = spec["align"].as_str() {
                    stack_options.align = Some(match a {
                        "start" => StackAlign::Start,
                        "center" => StackAlign::Center,
                        "end" => StackAlign::End,
                        _ => StackAlign::Stretch,
                    });
                }
                let comp: ComponentRef = if type_ == "h-stack" {
                    Rc::new(RefCell::new(HStack::new(children, stack_options)))
                } else {
                    Rc::new(RefCell::new(VStack::new(children, stack_options)))
                };
                Ok(Built::Plain(comp))
            }
            "loader" => {
                let indicator =
                    if spec.get("frames").is_some() || spec.get("intervalMs").is_some() {
                        Some(LoaderIndicatorOptions {
                            frames: spec["frames"].as_array().map(|a| {
                                a.iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect()
                            }),
                            interval_ms: spec["intervalMs"].as_u64(),
                        })
                    } else {
                        None
                    };
                let loader = Rc::new(RefCell::new(Loader::new(
                    None,
                    style_preset(),
                    style_preset(),
                    spec["message"].as_str().unwrap_or("Loading..."),
                    indicator,
                )));
                // Register with the virtual-clock registry so `ticks` steps
                // fire nested loaders too (TS: a real event loop fires ALL
                // armed setInterval callbacks, not just the root component).
                register_loader(&loader);
                Ok(Built::Loader(loader))
            }
            "cancellable-loader" => {
                let loader = Rc::new(RefCell::new(CancellableLoader::new(
                    None,
                    style_preset(),
                    style_preset(),
                    spec["message"].as_str().unwrap_or("Loading..."),
                    None,
                )));
                register_loader(&loader);
                if spec["onAbort"].as_str() == Some("record") {
                    let log2 = Rc::clone(log);
                    loader.borrow_mut().on_abort = Some(Box::new(move || {
                        log2.borrow_mut().push(serde_json::json!({ "kind": "abort" }));
                    }));
                }
                Ok(Built::CancelLoader(loader))
            }
            "mouse-region" => {
                let child = build(&spec["child"], log)?.comp();
                let handler: Box<
                    dyn Fn(&TuiMouseEvent) -> Option<pi_tui::component::TuiMouseEventResult>,
                > = if spec["onMouse"].as_str() == Some("record") {
                    let log2 = Rc::clone(log);
                    Box::new(move |ev: &TuiMouseEvent| {
                        log2.borrow_mut().push(mouse_event_json(ev));
                        None
                    })
                } else {
                    Box::new(|_ev: &TuiMouseEvent| None)
                };
                Ok(Built::Plain(Rc::new(RefCell::new(MouseRegion::new(
                    child, handler,
                )))))
            }
            "scroll-view" => {
                let mut options = ScrollViewOptions::default();
                match spec["follow"].as_str() {
                    Some("end") => options.follow = Some(ScrollViewFollow::End),
                    Some("none") => options.follow = Some(ScrollViewFollow::None),
                    _ => {}
                }
                if let Some(p) = spec["primary"].as_bool() {
                    options.primary = p;
                }
                Ok(Built::Scroll(Rc::new(RefCell::new(ScrollView::new(
                    build(&spec["child"], log)?.comp(),
                    options,
                )))))
            }
            "input" => {
                let mut options = InputOptions::default();
                if let Some(p) = spec["prompt"].as_str() {
                    options.prompt = Some(p.to_string());
                }
                if let Some(p) = spec["placeholder"].as_str() {
                    options.placeholder = Some(p.to_string());
                }
                let input = Rc::new(RefCell::new(Input::with_options(options)));
                if spec["focused"].as_bool().unwrap_or(false) {
                    input.borrow_mut().set_focused(true);
                }
                if spec["onSubmit"].as_str() == Some("record") {
                    let log2 = Rc::clone(log);
                    input.borrow_mut().on_submit = Some(Box::new(move |value: &str| {
                        log2.borrow_mut().push(
                            serde_json::json!({ "kind": "submit", "value": value }),
                        );
                    }));
                }
                if spec["onEscape"].as_str() == Some("record") {
                    let log2 = Rc::clone(log);
                    input.borrow_mut().on_escape = Some(Box::new(move || {
                        log2.borrow_mut().push(serde_json::json!({ "kind": "escape" }));
                    }));
                }
                Ok(Built::Input(input))
            }
            "select-list" => {
                let mut items: Vec<SelectItem> = Vec::new();
                let empty: Vec<Value> = Vec::new();
                for item in spec["items"].as_array().unwrap_or(&empty) {
                    let mut si = SelectItem::new(
                        item["value"].as_str().unwrap_or(""),
                        item["label"].as_str().unwrap_or(""),
                    );
                    if let Some(d) = item["description"].as_str() {
                        si = si.with_description(d);
                    }
                    items.push(si);
                }
                let mut layout = SelectListLayoutOptions::default();
                if let Some(lo) = spec["layoutOptions"].as_object() {
                    layout.min_primary_column_width = lo
                        .get("minPrimaryColumnWidth")
                        .and_then(Value::as_u64)
                        .map(|v| v as usize);
                    layout.max_primary_column_width = lo
                        .get("maxPrimaryColumnWidth")
                        .and_then(Value::as_u64)
                        .map(|v| v as usize);
                }
                let list = Rc::new(RefCell::new(SelectList::with_layout(
                    items,
                    num("maxVisible", 5) as usize,
                    SelectListTheme::default(),
                    layout,
                )));
                if spec["onSelect"].as_str() == Some("record") {
                    let log2 = Rc::clone(log);
                    list.borrow_mut().on_select = Some(Box::new(move |item: &SelectItem| {
                        log2.borrow_mut().push(item_event_json("select", item));
                    }));
                }
                if spec["onCancel"].as_str() == Some("record") {
                    let log2 = Rc::clone(log);
                    list.borrow_mut().on_cancel = Some(Box::new(move || {
                        log2.borrow_mut().push(serde_json::json!({ "kind": "cancel" }));
                    }));
                }
                if spec["onSelectionChange"].as_str() == Some("record") {
                    let log2 = Rc::clone(log);
                    list.borrow_mut().on_selection_change =
                        Some(Box::new(move |item: &SelectItem| {
                            log2.borrow_mut().push(item_event_json("selectionChange", item));
                        }));
                }
                Ok(Built::Select(list))
            }
            "settings-list" => {
                let mut items: Vec<SettingItem> = Vec::new();
                let empty: Vec<Value> = Vec::new();
                for item in spec["items"].as_array().unwrap_or(&empty) {
                    let mut si = SettingItem::new(
                        item["id"].as_str().unwrap_or(""),
                        item["label"].as_str().unwrap_or(""),
                        item["currentValue"].as_str().unwrap_or(""),
                    );
                    if let Some(d) = item["description"].as_str() {
                        si = si.with_description(d);
                    }
                    if let Some(values) = item["values"].as_array() {
                        si = si.with_values(
                            values
                                .iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect(),
                        );
                    }
                    items.push(si);
                }
                let log_change = Rc::clone(log);
                let on_change: Box<dyn Fn(&str, &str)> =
                    if spec["onChange"].as_str() == Some("record") {
                        Box::new(move |id: &str, value: &str| {
                            log_change.borrow_mut().push(
                                serde_json::json!({ "kind": "change", "id": id, "value": value }),
                            );
                        })
                    } else {
                        Box::new(|_, _| {})
                    };
                let log_cancel = Rc::clone(log);
                let on_cancel: Box<dyn Fn()> = if spec["onCancel"].as_str() == Some("record") {
                    Box::new(move || {
                        log_cancel
                            .borrow_mut()
                            .push(serde_json::json!({ "kind": "cancel" }));
                    })
                } else {
                    Box::new(|| {})
                };
                let mut options = SettingsListOptions::default();
                options.enable_search = Some(spec["enableSearch"].as_bool().unwrap_or(false));
                Ok(Built::Plain(Rc::new(RefCell::new(
                    SettingsList::with_options(
                        items,
                        num("maxVisible", 5) as usize,
                        SettingsListTheme::default(),
                        on_change,
                        on_cancel,
                        options,
                    ),
                ))))
            }
            other => return Err(format!("unknown tui-components type: {}", other)),
        }
    }

    let cases = input["cases"].as_array().ok_or("missing cases array")?;
    let mut results = Vec::new();
    for case in cases {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let built = build(&case["component"], &log)?;
        let comp = built.comp();
        let mut frames: Vec<Value> = Vec::new();
        let mut last_width: usize = 40;
        {
            let empty: Vec<Value> = Vec::new();
            for w in case["widths"].as_array().unwrap_or(&empty) {
                let width = w.as_u64().unwrap_or(40) as usize;
                last_width = width;
                let lines = comp.borrow_mut().render(width);
                frames.push(serde_json::json!({ "width": width, "lines": lines }));
            }
        }
        {
            let empty: Vec<Value> = Vec::new();
            for step in case["script"].as_array().unwrap_or(&empty) {
                {
                    let empty: Vec<Value> = Vec::new();
                    for s in step["inputs"].as_array().unwrap_or(&empty) {
                        let data = s.as_str().unwrap_or("");
                        if comp.borrow().handles_input() {
                            comp.borrow_mut().handle_input(data);
                        }
                    }
                }
                for _ in 0..step["ticks"].as_u64().unwrap_or(0) {
                    // Fire EVERY live armed loader (registered above), not
                    // just a top-level one — TS's patched setInterval store
                    // holds one callback per armed loader and the harness
                    // tick invokes all of them.
                    pi_tui::components::loader::tick_all_live_loaders();
                }
                if let Some(filter) = step["setFilter"].as_str() {
                    if let Built::Select(list) = &built {
                        list.borrow_mut().set_filter(filter);
                    }
                }
                {
                    let empty: Vec<Value> = Vec::new();
                    for op in step["scroll"].as_array().unwrap_or(&empty) {
                        if let Built::Scroll(view) = &built {
                            let sv = view.borrow_mut();
                            if let Some(ul) = op["updateLayout"].as_array() {
                                let ch = ul.get(0).and_then(Value::as_i64).unwrap_or(0);
                                let vh = ul.get(1).and_then(Value::as_i64).unwrap_or(0);
                                let mut noop = || {};
                                sv.update_layout(ch, vh, &mut noop);
                            }
                            if let Some(n) = op["scrollTo"].as_i64() {
                                sv.scroll_to(
                                    n,
                                    ScrollViewScrollToOptions {
                                        disable_follow: op["disableFollow"]
                                            .as_bool()
                                            .unwrap_or(false),
                                    },
                                );
                            }
                            if let Some(n) = op["scrollBy"].as_i64() {
                                sv.scroll_by(n);
                            }
                            if op["scrollToStart"].as_bool().unwrap_or(false) {
                                sv.scroll_to_start();
                            }
                            if op["scrollToEnd"].as_bool().unwrap_or(false) {
                                sv.scroll_to_end();
                            }
                        }
                    }
                }
                if let Some(m) = step["mouse"].as_object() {
                    let event_type = match m.get("type").and_then(Value::as_str) {
                        Some("release") => TuiMouseEventType::Release,
                        Some("move") => TuiMouseEventType::Move,
                        Some("drag") => TuiMouseEventType::Drag,
                        Some("click") => TuiMouseEventType::Click,
                        Some("wheel") => TuiMouseEventType::Wheel,
                        _ => TuiMouseEventType::Press,
                    };
                    let button = match m.get("button").and_then(Value::as_str) {
                        Some("left") => TuiMouseButton::Left,
                        Some("middle") => TuiMouseButton::Middle,
                        Some("right") => TuiMouseButton::Right,
                        _ => TuiMouseButton::None,
                    };
                    let x = m.get("x").and_then(Value::as_i64).unwrap_or(0);
                    let y = m.get("y").and_then(Value::as_i64).unwrap_or(0);
                    let width = m.get("width").and_then(Value::as_i64).unwrap_or(0);
                    let height = m.get("height").and_then(Value::as_i64).unwrap_or(0);
                    let event = TuiMouseEvent {
                        event_type,
                        button,
                        x,
                        y,
                        screen_x: x,
                        screen_y: y,
                        width,
                        height,
                        shift: m.get("shift").and_then(Value::as_bool).unwrap_or(false),
                        alt: m.get("alt").and_then(Value::as_bool).unwrap_or(false),
                        ctrl: m.get("ctrl").and_then(Value::as_bool).unwrap_or(false),
                        wheel_delta: m.get("wheelDelta").and_then(Value::as_i64),
                        click_count: m.get("clickCount").and_then(Value::as_i64),
                    };
                    let result = match dispatch_mouse_event(&comp, &event) {
                        None => Value::Null,
                        Some(r) => serde_json::json!({
                            "handled": r.handled,
                            "capture": r.capture,
                            "focus": r.focus,
                            "render": r.render,
                        }),
                    };
                    log.borrow_mut()
                        .push(serde_json::json!({ "kind": "dispatch", "result": result }));
                }
                if step["render"].as_bool().unwrap_or(false) {
                    let width = step["width"]
                        .as_u64()
                        .map(|w| w as usize)
                        .unwrap_or(last_width);
                    last_width = width;
                    let lines = comp.borrow_mut().render(width);
                    frames.push(serde_json::json!({ "width": width, "lines": lines }));
                }
            }
        }
        let mut state = serde_json::Map::new();
        match case["component"]["type"].as_str() {
            Some("input") => {
                if let Built::Input(input) = &built {
                    state.insert(
                        "value".to_string(),
                        serde_json::json!(input.borrow().get_value()),
                    );
                }
            }
            Some("select-list") => {
                if let Built::Select(list) = &built {
                    let selected = list.borrow().get_selected_item().map(|item| {
                        serde_json::json!({
                            "value": item.value,
                            "label": item.label,
                            "description": item.description,
                        })
                    });
                    state.insert("selected".to_string(), selected.unwrap_or(Value::Null));
                }
            }
            Some("cancellable-loader") => {
                if let Built::CancelLoader(loader) = &built {
                    state.insert(
                        "aborted".to_string(),
                        serde_json::json!(loader.borrow().aborted()),
                    );
                }
            }
            Some("scroll-view") => {
                if let Built::Scroll(view) = &built {
                    state.insert(
                        "scrollTop".to_string(),
                        serde_json::json!(view.borrow().scroll_top()),
                    );
                    state.insert(
                        "viewportHeight".to_string(),
                        serde_json::json!(view.borrow().viewport_height()),
                    );
                    state.insert(
                        "followingEnd".to_string(),
                        serde_json::json!(view.borrow().is_following_end()),
                    );
                }
            }
            _ => {}
        }
        results.push(serde_json::json!({
            "name": case["name"].as_str().unwrap_or(""),
            "frames": frames,
            "events": Value::Array(std::mem::take(&mut *log.borrow_mut())),
            "state": Value::Object(state),
        }));
    }
    Ok(serde_json::json!({ "results": results }))
}

/// `tui-editor` module-level harness entry: differential event-sequence
/// testing of the Editor component (mirrored on the TS side by
/// ts-harness.js's `tui-editor` handler; case contract in
/// docs/T4_PLAN.md §6).
///
/// Input: ONE case — `{ name, group: "tui-editor", theme: "default",
/// width?: 80, options?: { paddingX, autocompleteMaxVisible },
/// history?: string[], setText?: string, focused?: bool,
/// disableSubmit?: bool, script: [{ input } | { setText } |
/// { insertTextAtCursor } | { addToHistory } | { render: width|true } |
/// { tickMs: n }] }`.
///
/// Both sides construct `Editor` with the identity theme and a fake TUI
/// (`{ requestRender(){count++}, terminal: {rows: 24} }`), attach onChange /
/// onSubmit recorders, run the script, and return
/// `{ name, text, expandedText, cursor: {line, col}, lines,
/// frames: [{width, lines}], renderRequests, events: {onChange, onSubmit} }`.
///
/// Timer model: `{tickMs: n}` advances a deterministic virtual clock. The
/// TS side patches setTimeout and fires due callbacks (which START a
/// debounced autocomplete request but never resolve its promise chain —
/// the harness script contains no `await`). The Rust mirror is
/// `Editor::tick_autocomplete_debounce(n)`: with no provider attached
/// (no harness case sets one) `requestAutocomplete` returns at the
/// provider guard, so both sides are timer no-ops; the request-flush
/// divergence is therefore unobservable in this group.
fn run_tui_editor_harness(input: &Value) -> Result<Value, String> {
    use std::cell::RefCell;
    use std::rc::Rc;

    use pi_tui::component::{Component, Focusable, TuiHost};
    use pi_tui::components::editor::{Editor, EditorOptions, EditorTheme};

    /// The fake TUI (ts-harness.js `tui-editor`: `{ requestRender() {
    /// this.renderRequests++ }, terminal: { rows: 24, cols: 80 } }`).
    struct HarnessTui {
        render_requests: usize,
    }

    impl TuiHost for HarnessTui {
        fn request_render(&mut self) {
            self.render_requests += 1;
        }

        fn terminal_rows(&self) -> usize {
            24
        }
    }

    let options = if input.get("options").map(Value::is_null).unwrap_or(true) {
        EditorOptions::default()
    } else {
        let o = input["options"].as_object().ok_or("options must be an object")?;
        EditorOptions {
            padding_x: o.get("paddingX").and_then(Value::as_f64),
            autocomplete_max_visible: o.get("autocompleteMaxVisible").and_then(Value::as_f64),
        }
    };

    // Keep the typed handle for reading the render counter; the erased
    // copy handed to the Editor shares the same allocation (unsizing
    // coercion), so both observe the same `render_requests`.
    let tui = Rc::new(RefCell::new(HarnessTui { render_requests: 0 }));
    let host: Rc<RefCell<dyn TuiHost>> = tui.clone();
    let mut editor = Editor::with_options(host, EditorTheme::default(), options);

    // onChange / onSubmit recorders (the TS harness pushes into arrays).
    let on_change: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let on_submit: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let log = on_change.clone();
        editor.on_change = Some(Box::new(move |text: &str| {
            log.borrow_mut().push(text.to_string());
        }));
    }
    {
        let log = on_submit.clone();
        editor.on_submit = Some(Box::new(move |text: &str| {
            log.borrow_mut().push(text.to_string());
        }));
    }

    if input["focused"].as_bool().unwrap_or(false) {
        editor.set_focused(true);
    }
    if input["disableSubmit"].as_bool().unwrap_or(false) {
        editor.disable_submit = true;
    }
    if let Some(history) = input["history"].as_array() {
        for h in history {
            let text = h.as_str().ok_or("history entries must be strings")?;
            editor.add_to_history(text);
        }
    }
    if let Some(set_text) = input["setText"].as_str() {
        editor.set_text(set_text);
    }

    let default_width = input["width"].as_u64().unwrap_or(80) as usize;
    let mut frames: Vec<Value> = Vec::new();
    let script = input["script"].as_array().cloned().unwrap_or_default();
    for op in &script {
        let obj = op.as_object().ok_or("script op must be an object")?;
        if let Some(data) = obj.get("input").and_then(Value::as_str) {
            editor.handle_input(data);
        } else if let Some(text) = obj.get("setText").and_then(Value::as_str) {
            editor.set_text(text);
        } else if let Some(text) = obj.get("insertTextAtCursor").and_then(Value::as_str) {
            editor.insert_text_at_cursor(text);
        } else if let Some(text) = obj.get("addToHistory").and_then(Value::as_str) {
            editor.add_to_history(text);
        } else if let Some(render) = obj.get("render") {
            // `{render: true}` renders at the case's default width.
            let width = match render {
                Value::Bool(true) => default_width,
                v => v.as_u64().ok_or("render must be true or a width")? as usize,
            };
            let lines = Component::render(&mut editor, width);
            frames.push(serde_json::json!({ "width": width, "lines": lines }));
        } else if let Some(tick) = obj.get("tickMs") {
            let ms = tick.as_u64().ok_or("tickMs must be a number")?;
            editor.tick_autocomplete_debounce(ms);
        } else {
            return Err(format!(
                "invalid tui-editor script op: {:?}",
                obj.keys().map(|k| k.as_str()).collect::<Vec<_>>()
            ));
        }
    }

    let (cursor_line, cursor_col) = editor.get_cursor();
    Ok(serde_json::json!({
        "name": input["name"].as_str().unwrap_or(""),
        "text": editor.get_text(),
        "expandedText": editor.get_expanded_text(),
        "cursor": { "line": cursor_line, "col": cursor_col },
        "lines": editor.get_lines(),
        "frames": frames,
        "renderRequests": tui.borrow().render_requests,
        "events": {
            "onChange": *on_change.borrow(),
            "onSubmit": *on_submit.borrow(),
        },
    }))
}

// =============================================================================
// tui-main / tui-alt harness (T5 Wave C)
// =============================================================================

/// Input: ONE case — `{ name, cols?, rows?, capabilities?, showHardwareCursor?,
/// clearOnShrink?, altOptions? (alt), script: [op...], screenLines? (alt) }`.
///
/// Mirrors the TS-side `tui-main` / `tui-alt` ts-harness handlers 1:1:
/// - `RecordingTerminal` implements the `Terminal` trait with the same
///   observability as upstream `VirtualTerminal` (only `write()` recorded,
///   hide/showCursor counted, kitty protocol always active).
/// - Every built component is registered under an auto ref (`c0`, `c1`, …)
///   in build order (children before parents) so `setFocus` and the
///   `focused` output can name components.
/// - Script ops: addChild, input, terminalReply, setFocus, showOverlay,
///   hideOverlay, requestRender, renderNow, resize, tickMs, start, stop,
///   and (alt-only) setLayoutRoot, scrollBy, flash.
/// - Output: `{ name, frames, writes, fullRedraws, hideCursor, showCursor,
///   focused }` plus (alt) `{ viewportTop, followingOutput, screenLines? }`.
///   `frames` = the concatenated write stream split at every synchronized-
///   output begin marker (`\x1b[?2026h`) — one segment per render.
///
/// Environment: the case process runs with the TUI-relevant env vars
/// removed (TERM/TMUX/ZELLIJ/STY/TERMUX_VERSION/WEZTERM_PANE/TERM_PROGRAM/
/// SSH_*/PI_*) and capabilities pinned via `set_capabilities` — the TS side
/// does the same, so env detection is deterministic on both sides.
fn run_tui_screen_harness(input: &Value, alt: bool) -> Result<Value, String> {
    use std::cell::RefCell;
    use std::rc::Rc;

    use pi_tui::clock;
    use pi_tui::component::ComponentRef;
    use pi_tui::components::box_component::TuiBox;
    use pi_tui::components::cancellable_loader::CancellableLoader;
    use pi_tui::components::h_stack::HStack;
    use pi_tui::components::input::{Input, InputOptions};
    use pi_tui::components::loader::{Loader, LoaderIndicatorOptions};
    use pi_tui::components::mouse_region::MouseRegion;
    use pi_tui::components::scroll_view::{ScrollView, ScrollViewFollow, ScrollViewOptions};
    use pi_tui::components::select_list::{SelectItem, SelectList, SelectListTheme};
    use pi_tui::components::settings_list::{
        SettingItem, SettingsList, SettingsListOptions, SettingsListTheme,
    };
    use pi_tui::components::spacer::Spacer;
    use pi_tui::components::stack::{StackChild, StackEntryOptions, StackOptions};
    use pi_tui::components::text::{CustomBgFn, Text};
    use pi_tui::components::truncated_text::TruncatedText;
    use pi_tui::components::v_stack::VStack;
    use pi_tui::layout::ScrollViewScrollbar;
    use pi_tui::layout_node::Overscroll;
    use pi_tui::layout_node::{Basis, StackAlign};
    use pi_tui::terminal::{InputHandler, ProgramStatus, ResizeHandler, Terminal};
    use pi_tui::terminal_image::{set_capabilities, ImageProtocol, TerminalCapabilities};
    use pi_tui::component::Focusable;
    use pi_tui::tui::{
        OverlayAnchor, OverlayMargin, OverlayMarginOrNumber, OverlayOptions, SizeValue, TuiSurface,
        TuiStopOptions,
    };
    use pi_tui::tui_alt_screen::{TuiAltScreen, TuiAltScreenOptions};
    use pi_tui::tui_main_screen::TuiMainScreen;
    use pi_tui::wheel_scroll::WheelScrollLines;

    // --- deterministic environment (mirrors the TS handler's ENV_KEYS) ---
    for key in [
        "TERM", "TMUX", "ZELLIJ", "STY", "TERMUX_VERSION", "WEZTERM_PANE", "TERM_PROGRAM",
        "COLORTERM", "SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY",
        "PI_IMAGE_PROTOCOL", "PI_TRUE_COLOR", "PI_HYPERLINKS",
        "PI_TUI_DEBUG", "PI_TUI_DEBUG_REDRAW",
    ] {
        std::env::remove_var(key);
    }

    // --- capabilities pin ---
    let caps = input.get("capabilities");
    let default_caps = serde_json::json!({ "images": null, "trueColor": true, "hyperlinks": true });
    let caps = caps.unwrap_or(&default_caps);
    let images = match caps["images"].as_str() {
        Some("kitty") => Some(ImageProtocol::Kitty),
        Some("iterm2") => Some(ImageProtocol::Iterm2),
        _ => None,
    };
    set_capabilities(TerminalCapabilities {
        images,
        true_color: caps["trueColor"].as_bool().unwrap_or(true),
        hyperlinks: caps["hyperlinks"].as_bool().unwrap_or(true),
    });

    clock::set_now_ms(0.0);

    // --- RecordingTerminal (VirtualTerminal observability) ---
    struct RecordingTerminal {
        cols: usize,
        rows: usize,
        writes: Vec<String>,
        hide_count: u64,
        show_count: u64,
        input_handler: Option<InputHandler>,
        resize_handler: Option<ResizeHandler>,
    }
    impl Terminal for RecordingTerminal {
        fn start(&mut self, on_input: InputHandler, on_resize: ResizeHandler) {
            self.input_handler = Some(on_input);
            self.resize_handler = Some(on_resize);
        }
        fn stop(&mut self) {
            self.input_handler = None;
            self.resize_handler = None;
        }
        fn drain_input(&mut self, _max_ms: u64, _idle_ms: u64) {}
        fn write(&mut self, data: &str) {
            self.writes.push(data.to_string());
        }
        fn columns(&self) -> usize {
            self.cols
        }
        fn rows(&self) -> usize {
            self.rows
        }
        fn kitty_protocol_active(&self) -> bool {
            true
        }
        fn move_by(&mut self, _lines: i32) {}
        fn hide_cursor(&mut self) {
            self.hide_count += 1;
        }
        fn show_cursor(&mut self) {
            self.show_count += 1;
        }
        fn clear_line(&mut self) {}
        fn clear_from_cursor(&mut self) {}
        fn clear_screen(&mut self) {}
        fn set_title(&mut self, _title: &str) {}
        fn set_progress(&mut self, _active: bool) {}
        fn set_program_status(&mut self, _status: ProgramStatus) {}
    }

    let cols = input["cols"].as_u64().unwrap_or(80) as usize;
    let rows = input["rows"].as_u64().unwrap_or(24) as usize;
    let term = Rc::new(RefCell::new(RecordingTerminal {
        cols,
        rows,
        writes: Vec::new(),
        hide_count: 0,
        show_count: 0,
        input_handler: None,
        resize_handler: None,
    }));
    let term_dyn: Rc<RefCell<dyn Terminal>> = term.clone();

    // --- component spec builder (tui-components vocabulary; children are
    // registered before parents, matching the TS build order) ---
    fn bg_preset(preset: Option<&str>) -> Option<CustomBgFn> {
        match preset {
            Some("identity") => Some(Box::new(|t: &str| t.to_string())),
            Some("bracket") => Some(Box::new(|t: &str| format!("[{t}]"))),
            _ => None,
        }
    }
    fn build_screen_component(
        spec: &Value,
        registry: &mut Vec<ComponentRef>,
    ) -> Result<ComponentRef, String> {
        let type_ = spec["type"].as_str().ok_or("missing component type")?;
        let num = |key: &str, default: u64| -> u64 {
            spec[key].as_f64().map(|f| f.max(0.0) as u64).unwrap_or(default)
        };
        let register = |registry: &mut Vec<ComponentRef>, comp: ComponentRef| {
            registry.push(comp.clone());
            comp
        };
        let comp: ComponentRef = match type_ {
            "text" => register(
                registry,
                Rc::new(RefCell::new(Text::new(
                    spec["text"].as_str().unwrap_or(""),
                    num("paddingX", 1) as usize,
                    num("paddingY", 1) as usize,
                    bg_preset(spec["bg"].as_str()),
                ))),
            ),
            "truncated-text" => register(
                registry,
                Rc::new(RefCell::new(TruncatedText::new(
                    spec["text"].as_str().unwrap_or(""),
                    num("paddingX", 0) as usize,
                    num("paddingY", 0) as usize,
                ))),
            ),
            "spacer" => register(
                registry,
                Rc::new(RefCell::new(Spacer::new(num("lines", 1) as usize))),
            ),
            "box" => {
                let mut box_ = TuiBox::new(
                    num("paddingX", 1) as usize,
                    num("paddingY", 1) as usize,
                    bg_preset(spec["bg"].as_str()),
                );
                let empty: Vec<Value> = Vec::new();
                for child in spec["children"].as_array().unwrap_or(&empty) {
                    box_.add_child(build_screen_component(child, registry)?);
                }
                register(registry, Rc::new(RefCell::new(box_)))
            }
            "h-stack" | "v-stack" => {
                let mut children: Vec<StackChild> = Vec::new();
                let empty: Vec<Value> = Vec::new();
                for entry in spec["entries"].as_array().unwrap_or(&empty) {
                    let component = build_screen_component(&entry["component"], registry)?;
                    let mut options = StackEntryOptions::default();
                    if entry["basis"].as_str() == Some("auto") {
                        options.basis = Some(Basis::Auto);
                    } else if let Some(b) = entry["basis"].as_f64() {
                        options.basis = Some(Basis::Points(b));
                    }
                    if let Some(g) = entry["grow"].as_f64() {
                        options.grow = Some(g);
                    }
                    if let Some(s) = entry["shrink"].as_f64() {
                        options.shrink = Some(s);
                    }
                    if let Some(m) = entry["minSize"].as_f64() {
                        options.min_size = Some(m);
                    }
                    if let Some(m) = entry["maxSize"].as_f64() {
                        options.max_size = Some(m);
                    }
                    match entry["visible"].as_str() {
                        Some("always") => {
                            options.visible =
                                Some(Rc::new(|_vp: &pi_tui::layout_node::LayoutViewport| true));
                        }
                        // TS maps ANY other present value to never-visible.
                        _ if entry.get("visible").is_some() => {
                            options.visible =
                                Some(Rc::new(|_vp: &pi_tui::layout_node::LayoutViewport| false));
                        }
                        _ => {}
                    }
                    children.push(StackChild::Entry { component, options });
                }
                let mut stack_options = StackOptions::default();
                if let Some(g) = spec["gap"].as_f64() {
                    stack_options.gap = Some(g);
                }
                if let Some(a) = spec["align"].as_str() {
                    stack_options.align = Some(match a {
                        "start" => StackAlign::Start,
                        "center" => StackAlign::Center,
                        "end" => StackAlign::End,
                        _ => StackAlign::Stretch,
                    });
                }
                register(
                    registry,
                    if type_ == "h-stack" {
                        Rc::new(RefCell::new(HStack::new(children, stack_options)))
                    } else {
                        Rc::new(RefCell::new(VStack::new(children, stack_options)))
                    },
                )
            }
            "loader" => {
                let indicator = if spec.get("frames").is_some() || spec.get("intervalMs").is_some() {
                    Some(LoaderIndicatorOptions {
                        frames: spec["frames"].as_array().map(|a| {
                            a.iter().filter_map(|v| v.as_str().map(String::from)).collect()
                        }),
                        interval_ms: spec["intervalMs"].as_u64(),
                    })
                } else {
                    None
                };
                let loader = Rc::new(RefCell::new(Loader::new(
                    None,
                    Box::new(|t: &str| t.to_string()),
                    Box::new(|t: &str| t.to_string()),
                    spec["message"].as_str().unwrap_or("Loading..."),
                    indicator,
                )));
                // TS arms a real setInterval at construction; the virtual
                // clock pump must fire it (see LoaderClock).
                pi_tui::components::loader::register_loader(&loader);
                register(registry, loader)
            }
            "cancellable-loader" => {
                let loader = Rc::new(RefCell::new(CancellableLoader::new(
                    None,
                    Box::new(|t: &str| t.to_string()),
                    Box::new(|t: &str| t.to_string()),
                    spec["message"].as_str().unwrap_or("Loading..."),
                    None,
                )));
                pi_tui::components::loader::register_loader(&loader);
                register(registry, loader)
            }
            "mouse-region" => {
                let child = build_screen_component(&spec["child"], registry)?;
                register(
                    registry,
                    Rc::new(RefCell::new(MouseRegion::new(child, Box::new(
                        |_ev: &pi_tui::component::TuiMouseEvent| None,
                    )))),
                )
            }
            "scroll-view" => {
                let child = build_screen_component(&spec["child"], registry)?;
                let mut options = ScrollViewOptions::default();
                match spec["follow"].as_str() {
                    Some("end") => options.follow = Some(ScrollViewFollow::End),
                    Some("none") => options.follow = Some(ScrollViewFollow::None),
                    _ => {}
                }
                if let Some(p) = spec["primary"].as_bool() {
                    options.primary = p;
                }
                if let Some(o) = spec["overscroll"].as_str() {
                    options.overscroll = Some(match o {
                        "contain" => Overscroll::Contain,
                        _ => Overscroll::Chain,
                    });
                }
                if let Some(s) = spec["scrollbar"].as_str() {
                    options.scrollbar = Some(match s {
                        "hidden" => ScrollViewScrollbar::Hidden,
                        "always" => ScrollViewScrollbar::Always,
                        _ => ScrollViewScrollbar::Auto,
                    });
                }
                if spec["scrollbarTrackStyle"].as_str() == Some("identity") {
                    options.scrollbar_track_style =
                        Some(Rc::new(|t: &str| t.to_string()));
                }
                if spec["scrollbarThumbStyle"].as_str() == Some("identity") {
                    options.scrollbar_thumb_style =
                        Some(Rc::new(|t: &str| t.to_string()));
                }
                if let Some(d) = spec["scrollbarHideDelayMs"].as_f64() {
                    options.scrollbar_hide_delay_ms = Some(d);
                }
                register(registry, Rc::new(RefCell::new(ScrollView::new(child, options))))
            }
            "input" => {
                let mut options = InputOptions::default();
                if let Some(p) = spec["prompt"].as_str() {
                    options.prompt = Some(p.to_string());
                }
                if let Some(p) = spec["placeholder"].as_str() {
                    options.placeholder = Some(p.to_string());
                }
                let input = Rc::new(RefCell::new(Input::with_options(options)));
                if spec["focused"].as_bool().unwrap_or(false) {
                    input.borrow_mut().set_focused(true);
                }
                register(registry, input)
            }
            "select-list" => {
                let empty: Vec<Value> = Vec::new();
                let items: Vec<SelectItem> = spec["items"]
                    .as_array()
                    .unwrap_or(&empty)
                    .iter()
                    .map(|it| SelectItem {
                        value: it["value"].as_str().unwrap_or("").to_string(),
                        label: it["label"].as_str().unwrap_or("").to_string(),
                        description: it["description"].as_str().map(String::from),
                    })
                    .collect();
                register(
                    registry,
                    Rc::new(RefCell::new(SelectList::new(
                        items,
                        num("maxVisible", 5) as usize,
                        SelectListTheme::default(),
                    ))),
                )
            }
            "settings-list" => {
                let empty: Vec<Value> = Vec::new();
                let mut items: Vec<SettingItem> = Vec::new();
                for it in spec["items"].as_array().unwrap_or(&empty) {
                    let mut si = SettingItem::new(
                        it["id"].as_str().unwrap_or(""),
                        it["label"].as_str().unwrap_or(""),
                        it["currentValue"].as_str().unwrap_or(""),
                    );
                    if let Some(d) = it["description"].as_str() {
                        si = si.with_description(d);
                    }
                    if let Some(values) = it["values"].as_array() {
                        si = si.with_values(
                            values
                                .iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect(),
                        );
                    }
                    items.push(si);
                }
                let mut sl_options = SettingsListOptions::default();
                sl_options.enable_search = Some(spec["enableSearch"].as_bool().unwrap_or(false));
                register(
                    registry,
                    Rc::new(RefCell::new(SettingsList::with_options(
                        items,
                        num("maxVisible", 5) as usize,
                        SettingsListTheme::default(),
                        Box::new(|_, _| {}),
                        Box::new(|| {}),
                        sl_options,
                    ))),
                )
            }
            other => return Err(format!("unknown tui-main/tui-alt component type: {other}")),
        };
        Ok(comp)
    }

    fn parse_size_value_json(v: &Value) -> Result<SizeValue, String> {
        if let Some(n) = v.as_f64() {
            return Ok(SizeValue::Number(n));
        }
        if let Some(s) = v.as_str() {
            if let Some(num_part) = s.strip_suffix('%') {
                if num_part.parse::<f64>().is_ok() {
                    return Ok(SizeValue::Percent(s.to_string()));
                }
            }
        }
        Err(format!("invalid SizeValue: {v}"))
    }

    fn parse_overlay_options(v: Option<&Value>) -> Result<OverlayOptions, String> {
        let mut o = OverlayOptions::default();
        let Some(v) = v else { return Ok(o) };
        if let Some(x) = v.get("width") {
            o.width = Some(parse_size_value_json(x)?);
        }
        if let Some(x) = v.get("minWidth").and_then(|x| x.as_i64()) {
            o.min_width = Some(x);
        }
        if let Some(x) = v.get("maxHeight") {
            o.max_height = Some(parse_size_value_json(x)?);
        }
        if let Some(a) = v.get("anchor").and_then(|x| x.as_str()) {
            o.anchor = Some(match a {
                "center" => OverlayAnchor::Center,
                "top-left" => OverlayAnchor::TopLeft,
                "top-right" => OverlayAnchor::TopRight,
                "bottom-left" => OverlayAnchor::BottomLeft,
                "bottom-right" => OverlayAnchor::BottomRight,
                "top-center" => OverlayAnchor::TopCenter,
                "bottom-center" => OverlayAnchor::BottomCenter,
                "left-center" => OverlayAnchor::LeftCenter,
                "right-center" => OverlayAnchor::RightCenter,
                other => return Err(format!("unknown overlay anchor: {other}")),
            });
        }
        if let Some(x) = v.get("offsetX").and_then(|x| x.as_i64()) {
            o.offset_x = Some(x);
        }
        if let Some(x) = v.get("offsetY").and_then(|x| x.as_i64()) {
            o.offset_y = Some(x);
        }
        if let Some(x) = v.get("row") {
            o.row = Some(parse_size_value_json(x)?);
        }
        if let Some(x) = v.get("col") {
            o.col = Some(parse_size_value_json(x)?);
        }
        if let Some(m) = v.get("margin") {
            if let Some(n) = m.as_i64() {
                o.margin = Some(OverlayMarginOrNumber::Number(n));
            } else {
                o.margin = Some(OverlayMarginOrNumber::Margin(OverlayMargin {
                    top: m["top"].as_i64(),
                    right: m["right"].as_i64(),
                    bottom: m["bottom"].as_i64(),
                    left: m["left"].as_i64(),
                }));
            }
        }
        if v["visible"].as_str() == Some("always") {
            o.visible = Some(Rc::new(|_w: usize, _h: usize| true));
        }
        if let Some(nc) = v.get("nonCapturing").and_then(|x| x.as_bool()) {
            o.non_capturing = nc;
        }
        Ok(o)
    }

    // --- screen construction ---
    let show_hardware_cursor = input["showHardwareCursor"].as_bool();
    let mut alt_options = TuiAltScreenOptions::default();
    if let Some(o) = input.get("altOptions") {
        if let Some(w) = o.get("wheelScrollLines") {
            alt_options.wheel_scroll_lines = Some(match w {
                Value::String(s) if s == "auto" => WheelScrollLines::Auto,
                Value::Number(n) => WheelScrollLines::Lines(n.as_f64().unwrap_or(1.0)),
                _ => WheelScrollLines::Lines(1.0),
            });
        }
        if let Some(m) = o.get("mouse").and_then(|x| x.as_bool()) {
            alt_options.mouse = Some(m);
        }
        if o["searchMatchStyle"].as_str() == Some("identity") {
            alt_options.search_match_style = Some(Rc::new(|t: &str| t.to_string()));
        }
        if o["searchCurrentMatchStyle"].as_str() == Some("identity") {
            alt_options.search_current_match_style = Some(Rc::new(|t: &str| t.to_string()));
        }
        if let Some(c) = o.get("copyOnSelect").and_then(|x| x.as_bool()) {
            alt_options.copy_on_select = Some(c);
        }
    }

    let alt_rc: Option<Rc<RefCell<TuiAltScreen>>> = if alt {
        Some(TuiAltScreen::new(
            term_dyn.clone(),
            show_hardware_cursor,
            None,
            alt_options,
        ))
    } else {
        None
    };
    let main_rc: Option<Rc<RefCell<TuiMainScreen>>> = if alt {
        None
    } else {
        Some(TuiMainScreen::new(term_dyn.clone(), show_hardware_cursor, None))
    };
    let surface: Rc<RefCell<dyn TuiSurface>> = if let Some(rc) = &alt_rc {
        rc.clone()
    } else {
        main_rc.as_ref().expect("main screen").clone()
    };
    if input["clearOnShrink"].as_bool().unwrap_or(false) {
        surface.borrow_mut().set_clear_on_shrink(true);
    }

    // --- script interpreter ---
    let mut registry: Vec<ComponentRef> = Vec::new();
    let lookup_ref = |registry: &Vec<ComponentRef>, r: &str| -> Result<ComponentRef, String> {
        let idx: usize = r
            .strip_prefix('c')
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| format!("unknown component ref: {r}"))?;
        registry
            .get(idx)
            .cloned()
            .ok_or_else(|| format!("unknown component ref: {r}"))
    };

    let empty: Vec<Value> = Vec::new();
    for op in input["script"].as_array().unwrap_or(&empty) {
        let type_ = op["type"].as_str().ok_or("missing script op type")?;
        match type_ {
            "addChild" => {
                let component =
                    build_screen_component(&op["component"], &mut registry)?;
                surface.borrow_mut().add_child(component);
            }
            "input" | "terminalReply" => {
                let data = op["data"].as_str().unwrap_or("").to_string();
                let handler = term.borrow().input_handler.clone();
                if let Some(h) = handler {
                    let mut f = h.borrow_mut();
                    f(data);
                }
            }
            "setFocus" => {
                let target = if op["ref"].is_null() {
                    None
                } else {
                    let r = op["ref"].as_str().ok_or("setFocus ref must be a string or null")?;
                    Some(lookup_ref(&registry, r)?)
                };
                surface.borrow_mut().set_focus(target);
            }
            "showOverlay" => {
                let component =
                    build_screen_component(&op["component"], &mut registry)?;
                let options = parse_overlay_options(op.get("options"))?;
                surface.borrow_mut().show_overlay(component, options);
            }
            "hideOverlay" => {
                surface.borrow_mut().hide_overlay();
            }
            "requestRender" => {
                surface.borrow_mut().request_render(op["force"].as_bool().unwrap_or(false));
            }
            "renderNow" => {
                surface.borrow_mut().render_now(op["force"].as_bool().unwrap_or(false));
            }
            "resize" => {
                let cols = op["cols"].as_u64().ok_or("resize requires cols")? as usize;
                let rows = op["rows"].as_u64().ok_or("resize requires rows")? as usize;
                let handler = {
                    let mut t = term.borrow_mut();
                    if t.cols == cols && t.rows == rows {
                        None
                    } else {
                        t.cols = cols;
                        t.rows = rows;
                        t.resize_handler.clone()
                    }
                };
                if let Some(h) = handler {
                    let mut f = h.borrow_mut();
                    f();
                }
            }
            "tickMs" => {
                // TS harness: `clock += op.n` with a missing n yields NaN
                // and every deadline comparison stays false (no timers
                // ever fire again) — model that exactly rather than
                // erroring (fuzz/reg-main-043).
                let ms = op["n"].as_f64().unwrap_or(f64::NAN);
                surface.borrow_mut().advance_clock(ms);
            }
            "start" => {
                surface.borrow_mut().start();
            }
            "stop" => {
                let options = TuiStopOptions {
                    preserve_screen: op["preserveScreen"].as_bool().unwrap_or(false),
                };
                surface.borrow_mut().stop(&options);
            }
            "setLayoutRoot" => {
                let Some(alt_rc) = &alt_rc else {
                    return Err("setLayoutRoot is alt-only".into());
                };
                let component = if op["component"].is_null() {
                    None
                } else {
                    Some(build_screen_component(&op["component"], &mut registry)?)
                };
                alt_rc.borrow_mut().set_layout_root(component);
            }
            "scrollBy" => {
                let Some(alt_rc) = &alt_rc else {
                    return Err("scrollBy is alt-only".into());
                };
                let lines = op["lines"].as_i64().ok_or("scrollBy requires lines")?;
                alt_rc.borrow_mut().scroll_by(lines);
            }
            "flash" => {
                let Some(alt_rc) = &alt_rc else {
                    return Err("flash is alt-only".into());
                };
                let message = op["message"].as_str().unwrap_or("").to_string();
                let duration = op["durationMs"].as_f64();
                alt_rc.borrow_mut().flash(&message, duration);
            }
            other => return Err(format!("invalid tui-main/tui-alt script op: {other}")),
        }
        surface.borrow_mut().run_pending_ticks();
    }

    // --- output ---
    let (writes, hide_count, show_count) = {
        let t = term.borrow();
        (t.writes.clone(), t.hide_count, t.show_count)
    };
    let begin = "\x1b[?2026h";
    let stream: String = writes.concat();
    let pieces: Vec<&str> = stream.split(begin).collect();
    let mut frames: Vec<String> = Vec::new();
    if !pieces[0].is_empty() {
        frames.push(pieces[0].to_string());
    }
    for p in &pieces[1..] {
        frames.push(format!("{begin}{p}"));
    }
    let focused = {
        let f = surface.borrow().get_focused_component();
        f.and_then(|f| {
            registry
                .iter()
                .position(|c| Rc::ptr_eq(c, &f))
                .map(|idx| format!("c{idx}"))
        })
    };
    let full_redraws = surface.borrow().full_redraws();

    let mut out = serde_json::json!({
        "name": input["name"].as_str().unwrap_or(""),
        "frames": frames,
        "writes": writes,
        "fullRedraws": full_redraws,
        "hideCursor": hide_count,
        "showCursor": show_count,
        "focused": focused,
    });
    if let Some(alt_rc) = &alt_rc {
        out["viewportTop"] = serde_json::json!(alt_rc.borrow().viewport_top());
        out["followingOutput"] = serde_json::json!(alt_rc.borrow().is_following_output());
        if input["screenLines"].as_bool().unwrap_or(false) {
            out["screenLines"] = serde_json::json!(alt_rc.borrow().get_screen_lines());
        }
    }
    Ok(out)
}

/// Drive `pi-tools` `file_mutation_queue.rs` (mirrors
/// dist/core/tools/file-mutation-queue.js). args:
/// `{ setup?: {realFiles?: [{path, content?}], symlinks?: [{link, target}]},
///    ops: [{path, script: ["label" | {awaitGate} | {throw}]}],
///    release: [{gate, afterStarted: [op indices]}] }`
/// → `{ log: [...], outcomes: [...] }`. Ops run concurrently; each op's fn
/// marks itself started, then walks its script (label → log, awaitGate →
/// spin until the release rule fires, throw → fn error). Release rules are
/// processed in order once all their `afterStarted` ops have begun. Futures
/// are polled round-robin with a no-op waker (all synchronization is via
/// yield_now loops and the queue's async mutex), keeping the interleaving
/// deterministic and matching the TS microtask order.
fn run_file_mutation_queue_harness(input: &Value, cwd: &std::path::Path) -> Result<Value, String> {
    use pi_tools::file_mutation_queue::FileMutationQueues;
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex};
    use serde_json::json;
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Wake};

    struct NoopWake;
    impl Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }

    // setup: real files + symlinks, relative to the harness cwd
    if let Some(setup) = input["setup"].as_object() {
        for file in setup.get("realFiles").and_then(Value::as_array).unwrap_or(&Vec::new()) {
            let rel = file["path"].as_str().ok_or("setup.realFiles[].path must be a string")?;
            let target = cwd.join(rel);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let content = file["content"].as_str().unwrap_or("");
            std::fs::write(&target, content).map_err(|e| e.to_string())?;
        }
        for link in setup.get("symlinks").and_then(Value::as_array).unwrap_or(&Vec::new()) {
            let rel = link["link"].as_str().ok_or("setup.symlinks[].link must be a string")?;
            let target_rel =
                link["target"].as_str().ok_or("setup.symlinks[].target must be a string")?;
            let link_path = cwd.join(rel);
            let _ = std::fs::remove_file(&link_path);
            std::os::unix::fs::symlink(target_rel, &link_path).map_err(|e| e.to_string())?;
        }
    }

    let ops: Vec<Value> = input["ops"]
        .as_array()
        .cloned()
        .ok_or("missing ops")?;

    struct Shared {
        log: Mutex<Vec<String>>,
        started: Mutex<HashSet<usize>>,
        gates: Mutex<HashMap<String, bool>>,
        outcomes: Mutex<Vec<String>>,
    }
    let shared = Arc::new(Shared {
        log: Mutex::new(Vec::new()),
        started: Mutex::new(HashSet::new()),
        gates: Mutex::new(
            ops.iter()
                .filter_map(|op| {
                    op["script"].as_array().and_then(|steps| {
                        steps.iter().find_map(|s| s["awaitGate"].as_str().map(str::to_string))
                    })
                })
                .map(|gate| (gate, false))
                .collect::<HashMap<String, bool>>(),
        ),
        outcomes: Mutex::new(vec!["pending".to_string(); ops.len()]),
    });

    let queues = Arc::new(FileMutationQueues::new());

    // Each op future: claim its index, run with_queue, record outcome.
    let make_op = |i: usize| {
        let shared = Arc::clone(&shared);
        let queues = Arc::clone(&queues);
        let op = ops[i].clone();
        let cwd = cwd.to_path_buf();
        Box::pin(async move {
            let path = op["path"].as_str().unwrap_or_default().to_string();
            let script = op["script"].as_array().cloned().unwrap_or_default();
            let outcome = queues
                .with_queue(&path, &cwd, async {
                    shared.started.lock().expect("started").insert(i);
                    for step in &script {
                        if let Some(label) = step.as_str() {
                            shared.log.lock().expect("log").push(label.to_string());
                        } else if let Some(gate) = step["awaitGate"].as_str() {
                            loop {
                                if shared
                                    .gates
                                    .lock()
                                    .expect("gates")
                                    .get(gate)
                                    .copied()
                                    .unwrap_or(false)
                                {
                                    break;
                                }
                                tokio::task::yield_now().await;
                            }
                        } else if let Some(message) = step["throw"].as_str() {
                            return Err(message.to_string());
                        }
                    }
                    Ok(())
                })
                .await;
            let text = match outcome {
                Ok(Ok(())) => "ok".to_string(),
                Ok(Err(message)) => format!("error:{}", message),
                Err(key_error) => format!("keyError:{}", key_error),
            };
            shared.outcomes.lock().expect("outcomes")[i] = text;
        }) as Pin<Box<dyn Future<Output = ()>>>
    };

    // Master future: apply release rules in order.
    let master = {
        let shared = Arc::clone(&shared);
        let rules = input["release"].as_array().cloned().unwrap_or_default();
        Box::pin(async move {
            for rule in &rules {
                let gate = rule["gate"].as_str().unwrap_or_default().to_string();
                let need: Vec<usize> = rule["afterStarted"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_u64().map(|v| v as usize)).collect())
                    .unwrap_or_default();
                loop {
                    let started = shared.started.lock().expect("started");
                    if need.iter().all(|i| started.contains(i)) {
                        break;
                    }
                    drop(started);
                    tokio::task::yield_now().await;
                }
                shared.gates.lock().expect("gates").insert(gate, true);
            }
        }) as Pin<Box<dyn Future<Output = ()>>>
    };

    let mut futures: Vec<Pin<Box<dyn Future<Output = ()>>>> =
        (0..ops.len()).map(make_op).collect();
    futures.push(master);

    let waker = Arc::new(NoopWake).into();
    let mut cx = Context::from_waker(&waker);
    let mut done = vec![false; futures.len()];
    loop {
        let mut all_done = true;
        for (i, future) in futures.iter_mut().enumerate() {
            if done[i] {
                continue;
            }
            if future.as_mut().poll(&mut cx).is_pending() {
                all_done = false;
            } else {
                done[i] = true;
            }
        }
        if all_done {
            break;
        }
    }

    let outcomes = shared.outcomes.lock().expect("outcomes").clone();
    let log = shared.log.lock().expect("log").clone();
    Ok(json!({ "result": { "log": log, "outcomes": outcomes } }))
}
