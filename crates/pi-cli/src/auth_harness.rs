//! Parity-harness entries for the auth/composer wave tools:
//! `auth-storage`, `remote-catalog`.
//!
//! Wire protocol (TS mirror contract): each tool takes JSON args with a
//! `fn` discriminator and returns `{ result: ... }`; errors surface as
//! `{"error": msg}` via the `Err` path in `main.rs`.
//!
//! `auth-storage` fns:
//! - `readOnlyLoad` {content?} → parsed data | error (per-case temp file;
//!   absent content = missing file)
//! - `readOnlyRead` {content?, providerId, env?} → credential | null
//! - `readOnlyList` {content?} → [{providerId, type}]
//! - `readOnlyModify`/`readOnlyDelete` → always the read-only error
//! - `inMemoryBackend` {initial?: string|null, locks: [{result, next?}]} →
//!   {results, value} (backend withLock semantics; `next` absent = no write)
//! - `authStorageSequence` {data?: object, env?, steps:
//!   [{op: read|modify|delete|list|reload, providerId?, set?}]} over
//!   `AuthStorage::inMemory` → {results, content} (final backend string;
//!   `set` absent/null = the modify closure returning undefined)
//! - `readStoredCredential` {providerId, content?} → raw entry | null
//!
//! `remote-catalog` fns (no network: the response is injected via args;
//! `ok` is derived from status like a real Response):
//! - `withRemoteCatalog` {providerId, models, allModels?, localGeneratedAt?,
//!   stored?, allowNetwork, force?, now, response?:
//!   {status, lastModified?, etag?, body?}} →
//!   {getModels, getAllModels, publishes: [{persist, update}]} | error
//! - `withPersonalizedCatalog` (same + personalize: {op:
//!   identity|empty|reverse|ids, ids?}) → same shape
//!
//! The env argument is the parity stand-in for `process.env` (the TS
//! harness scrubs the ambient environment); command keys execute through
//! `/bin/sh` like the config-value tool. The User-Agent header is
//! normalized to `<user-agent>` on both sides.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use pi_providers::auth_storage::{
	AuthStorage, AuthStorageBackend, InMemoryAuthStorageBackend, LockResult,
	ReadOnlyAuthStorage, read_stored_credential,
};
use pi_providers::config_value::{CommandRunner, ConfigValueResolver, Env};
use pi_providers::remote_catalog as rc;
use pi_providers::remote_catalog::{PublishRecord, StoredCatalog};

pub fn run(tool: &str, input: &Value) -> Result<Value, String> {
	match tool {
		"auth-storage" => run_auth_storage(input),
		"remote-catalog" => run_remote_catalog(input),
		other => Err(format!("unknown auth tool: {other}")),
	}
}

// ============================================================================
// Shared helpers
// ============================================================================

/// Real shell execution for `!command` keys (mirrors TS `execSync`, 10 s
/// timeout, stdout trimmed, empty → None) — same as the config-value tool.
struct ShellCommandRunner;

impl CommandRunner for ShellCommandRunner {
	fn run(&self, command: &str) -> Option<String> {
		let mut child = Command::new("/bin/sh")
			.arg("-c")
			.arg(command)
			.stdin(Stdio::null())
			.stdout(Stdio::piped())
			.stderr(Stdio::null())
			.spawn()
			.ok()?;
		let deadline = Instant::now() + Duration::from_millis(10_000);
		loop {
			match child.try_wait() {
				Ok(Some(status)) if !status.success() => return None,
				Ok(Some(_)) => break,
				Ok(None) if Instant::now() >= deadline => {
					let _ = child.kill();
					let _ = child.wait();
					return None;
				}
				Ok(None) => std::thread::sleep(Duration::from_millis(5)),
				Err(_) => return None,
			}
		}
		let mut stdout = String::new();
		child.stdout.take()?.read_to_string(&mut stdout).ok()?;
		let trimmed = stdout.trim();
		(!trimmed.is_empty()).then(|| trimmed.to_string())
	}
}

fn parse_env(input: &Value) -> Result<Env, String> {
	let mut env = Env::new();
	if let Some(Value::Object(map)) = input.get("env") {
		if map.is_empty() {
			return Ok(env);
		}
		for (key, value) in map {
			let value = value
				.as_str()
				.ok_or_else(|| format!("non-string env value for {key}"))?;
			env.insert(key.clone(), value.to_string());
		}
	}
	Ok(env)
}

/// Per-case temp file (models-store precedent): writes `content` when
/// given, else leaves the file missing.
fn temp_auth_file(content: Option<&str>) -> Result<String, String> {
	static COUNTER: AtomicU64 = AtomicU64::new(0);
	let dir = std::env::temp_dir()
		.join("pi-parity-auth-storage")
		.join(std::process::id().to_string());
	std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
	let path = dir.join(format!("case-{}.json", COUNTER.fetch_add(1, Ordering::SeqCst)));
	match content {
		Some(content) => std::fs::write(&path, content).map_err(|e| e.to_string())?,
		None => {
			let _ = std::fs::remove_file(&path);
		}
	}
	Ok(path.to_string_lossy().into_owned())
}

fn content_arg(input: &Value) -> Option<&str> {
	input.get("content").and_then(Value::as_str)
}

// ============================================================================
// auth-storage
// ============================================================================

fn run_auth_storage(input: &Value) -> Result<Value, String> {
	let fn_name = input["fn"].as_str().ok_or("missing fn")?;
	match fn_name {
		"readOnlyLoad" => {
			let path = temp_auth_file(content_arg(input))?;
			let storage = ReadOnlyAuthStorage::new(
				path,
				ConfigValueResolver::new(&ShellCommandRunner),
				Env::new(),
			);
			match storage.load_data() {
				Ok(data) => Ok(json!({ "result": Value::Object(data) })),
				Err(error) => Err(error.message),
			}
		}
		"readOnlyRead" => {
			let path = temp_auth_file(content_arg(input))?;
			let provider_id = input["providerId"].as_str().ok_or("missing providerId")?;
			let storage = ReadOnlyAuthStorage::new(
				path,
				ConfigValueResolver::new(&ShellCommandRunner),
				parse_env(input)?,
			);
			let credential = storage
				.read_raw(provider_id, None)
				.map_err(|e| e.message)?;
			Ok(json!({ "result": credential.unwrap_or(Value::Null) }))
		}
		"readOnlyList" => {
			let path = temp_auth_file(content_arg(input))?;
			let storage = ReadOnlyAuthStorage::new(
				path,
				ConfigValueResolver::new(&ShellCommandRunner),
				Env::new(),
			);
			let entries = storage.list_raw(None).map_err(|e| e.message)?;
			Ok(json!({
				"result": entries
					.into_iter()
					.map(|(provider_id, ty)| json!({ "providerId": provider_id, "type": ty }))
					.collect::<Vec<_>>()
			}))
		}
		// The read-only store throws before touching storage.
		"readOnlyModify" | "readOnlyDelete" => {
			Err("Read-only credential storage cannot modify auth.json".to_string())
		}
		"inMemoryBackend" => {
			let initial = input
				.get("initial")
				.and_then(Value::as_str)
				.map(str::to_string);
			let backend = InMemoryAuthStorageBackend::new();
			if let Some(initial) = initial {
				let _ = backend.with_lock::<()>(
					&mut |_| Ok(LockResult { result: (), next: Some(initial.clone()) }),
					None,
				);
			}
			let locks = input["locks"].as_array().ok_or("missing locks")?;
			let mut results = Vec::new();
			for lock in locks {
				let next = lock.get("next").and_then(Value::as_str).map(str::to_string);
				let result = lock.get("result").cloned().unwrap_or(Value::Null);
				let outcome: Value = backend
					.with_lock(
						&mut |_| Ok(LockResult { result: result.clone(), next: next.clone() }),
						None,
					)
					.map_err(|e| e.message)?;
				results.push(outcome);
			}
			Ok(json!({ "result": { "results": results, "value": backend.value() } }))
		}
		"authStorageSequence" => {
			let data = input
				.get("data")
				.and_then(Value::as_object)
				.cloned()
				.unwrap_or_default();
			let storage = AuthStorage::<InMemoryAuthStorageBackend>::in_memory(
				data,
				ConfigValueResolver::new(&ShellCommandRunner),
				parse_env(input)?,
			);
			let steps = input["steps"].as_array().ok_or("missing steps")?;
			let mut results = Vec::new();
			for step in steps {
				let op = step["op"].as_str().ok_or("missing step op")?;
				let provider_id = step["providerId"].as_str().unwrap_or("");
				let result = match op {
					"read" => storage
						.read_raw(provider_id, None)
						.map_err(|e| e.message)?
						.unwrap_or(Value::Null),
					"modify" => {
						let set = match step.get("set") {
							Some(value) if !value.is_null() => Some(value.clone()),
							_ => None,
						};
						storage
							.modify_raw(
								provider_id,
								&mut move |_current: Option<&Value>| Ok(set.clone()),
								None,
							)
							.map_err(|e| e.message)?
							.unwrap_or(Value::Null)
					}
					"delete" => {
						storage.delete_raw(provider_id, None).map_err(|e| e.message)?;
						Value::Null
					}
					"list" => Value::Array(
						storage
							.list_raw(None)
							.map_err(|e| e.message)?
							.into_iter()
							.map(|(id, ty)| json!({ "providerId": id, "type": ty }))
							.collect(),
					),
					"reload" => {
						storage.reload();
						Value::Null
					}
					other => return Err(format!("unknown auth-storage step op: {other}")),
				};
				results.push(result);
			}
			Ok(json!({ "result": { "results": results, "content": storage.storage_ref().value() } }))
		}
		"readStoredCredential" => {
			let provider_id = input["providerId"].as_str().ok_or("missing providerId")?;
			let path = temp_auth_file(content_arg(input))?;
			Ok(json!({
				"result": read_stored_credential(provider_id, &path).unwrap_or(Value::Null)
			}))
		}
		other => Err(format!("unknown auth-storage fn: {other}")),
	}
}

// ============================================================================
// remote-catalog
// ============================================================================

struct RemoteCase {
	provider_id: String,
	models: Vec<Value>,
	all_models: Vec<Value>,
	local_generated_at: Option<f64>,
	stored: Option<StoredCatalog>,
	allow_network: bool,
	force: bool,
	now: f64,
	response: Option<Value>,
}

fn parse_remote_case(input: &Value) -> Result<RemoteCase, String> {
	let opt_number = |key: &str| -> Result<Option<f64>, String> {
		match input.get(key) {
			None | Some(Value::Null) => Ok(None),
			Some(value) => value.as_f64().map(Some).ok_or_else(|| format!("non-number {key}")),
		}
	};
	let provider_id = input["providerId"].as_str().ok_or("missing providerId")?.to_string();
	let models = input["models"].as_array().ok_or("missing models")?.clone();
	let all_models = match input.get("allModels") {
		Some(Value::Array(items)) => items.clone(),
		_ => models.clone(),
	};
	let stored = match input.get("stored") {
		None | Some(Value::Null) => None,
		Some(value) => Some(StoredCatalog::from_value(value)),
	};
	Ok(RemoteCase {
		provider_id,
		models,
		all_models,
		local_generated_at: opt_number("localGeneratedAt")?,
		stored,
		allow_network: input["allowNetwork"].as_bool().unwrap_or(true),
		force: input["force"].as_bool().unwrap_or(false),
		now: opt_number("now")?.unwrap_or(0.0),
		response: input.get("response").cloned().filter(|v| !v.is_null()),
	})
}

/// Build the scripted response (called lazily — a case without a response
/// only errors when the port actually performs a request).
fn response_from_spec(response: &Value) -> Result<rc::CatalogResponse, String> {
	let status = response["status"].as_u64().ok_or("missing response status")? as u16;
	Ok(rc::CatalogResponse {
		status,
		ok: (200..300).contains(&status),
		last_modified: response
			.get("lastModified")
			.and_then(Value::as_str)
			.map(str::to_string),
		etag: response.get("etag").and_then(Value::as_str).map(str::to_string),
		body: response.get("body").cloned().unwrap_or(Value::Null),
	})
}

impl RemoteCase {

	fn publishes_value(publishes: &[PublishRecord]) -> Value {
		Value::Array(
			publishes
				.iter()
				.map(|record| {
					json!({
						"persist": record.persist.clone().unwrap_or(Value::Null),
						"update": record.update,
					})
				})
				.collect(),
		)
	}
}

fn run_remote_catalog(input: &Value) -> Result<Value, String> {
	let fn_name = input["fn"].as_str().ok_or("missing fn")?;
	let case = parse_remote_case(input)?;
	match fn_name {
		"withRemoteCatalog" => {
			let mut dynamic = Vec::new();
			let mut publishes = Vec::new();
			let scripted = case.response.clone();
			let fetch = move |_req: &rc::CatalogRequest| match &scripted {
				Some(spec) => Ok::<_, pi_providers::error::JsError>(response_from_spec(spec)?),
				None => Err(pi_providers::error::JsError::new("remote-catalog: no scripted response")),
			};
			rc::refresh_with_remote_catalog(
				&mut dynamic,
				&mut publishes,
				&case.provider_id,
				rc::DEFAULT_CATALOG_BASE_URL,
				case.local_generated_at,
				case.stored.as_ref(),
				case.allow_network,
				case.force,
				case.now,
				&fetch,
				&mut || true,
			)
			.map_err(|e| e.message)?;
			Ok(json!({
				"result": {
					"getModels": rc::remote_catalog_get_models(&case.models, &dynamic),
					"getAllModels": rc::remote_catalog_get_all_models(&case.all_models, &dynamic),
					"publishes": RemoteCase::publishes_value(&publishes),
				}
			}))
		}
		"withPersonalizedCatalog" => {
			let personalize_spec = input.get("personalalize").or_else(|| input.get("personalize"));
			let op = personalize_spec
				.and_then(|spec| spec.get("op"))
				.and_then(Value::as_str)
				.unwrap_or("identity");
			let ids = personalize_spec
				.and_then(|spec| spec.get("ids"))
				.and_then(Value::as_array)
				.map(|ids| {
					ids.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>()
				})
				.unwrap_or_default();
			let personalize = move |catalog: &[Value]| -> Result<Vec<Value>, pi_providers::error::JsError> {
				Ok(match op {
					"empty" => Vec::new(),
					"reverse" => catalog.iter().rev().cloned().collect(),
					"ids" => catalog
						.iter()
						.filter(|m| m.get("id").and_then(Value::as_str).is_some_and(|id| ids.iter().any(|s| s == id)))
						.cloned()
						.collect(),
					_ => catalog.to_vec(),
				})
			};
			let mut personal = Vec::new();
			let mut publishes = Vec::new();
			let scripted = case.response.clone();
			let fetch = move |_req: &rc::CatalogRequest| match &scripted {
				Some(spec) => Ok::<_, pi_providers::error::JsError>(response_from_spec(spec)?),
				None => Err(pi_providers::error::JsError::new("remote-catalog: no scripted response")),
			};
			rc::refresh_with_personalized_catalog(
				&mut personal,
				&mut publishes,
				&case.provider_id,
				rc::DEFAULT_CATALOG_BASE_URL,
				&case.all_models,
				case.local_generated_at,
				case.stored.as_ref(),
				case.allow_network,
				case.now,
				&fetch,
				&personalize,
				&mut || true,
			)
			.map_err(|e| e.message)?;
			Ok(json!({
				"result": {
					"getModels": rc::personalized_get_models(&case.all_models, &personal),
					"getAllModels": rc::personalized_get_all_models(&case.all_models, &personal),
					"publishes": RemoteCase::publishes_value(&publishes),
				}
			}))
		}
		other => Err(format!("unknown remote-catalog fn: {other}")),
	}
}
