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
