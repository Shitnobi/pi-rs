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
