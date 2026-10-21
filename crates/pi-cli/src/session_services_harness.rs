//! Parity-harness entry for the `agent-session-services` protocol:
//! `pi_session::agent_session::services::create_agent_session_services`
//! (TS `core/agent-session-services.ts`) — pending registration drain
//! plus `applyExtensionFlagValues` over injected fakes. TS mirror:
//! parity/ts-harness.js `agent-session-services` entry (real
//! `DefaultResourceLoader` with inline extension factories; the Rust
//! fake loader mirrors the same case JSON).
//!
//! args:
//! ```json
//! {
//!   "options": { "cwd": string, "agentDir"?: string,
//!                "extensionFlagValues"?: [[name, bool|string], ...] },
//!   "extensions"?: [ { "name": string,
//!                      "flags"?: [{ "name": string,
//!                                   "type": "boolean"|"string" }],
//!                      "providers"?: [{ "name": string, "config"?: obj }],
//!                      "nativeProviders"?: [obj],
//!                      "virtualModels"?: [obj] } ],
//!   "modelRuntime"?: { "throwOn"?: [ "registerProvider" |
//!                                     "registerNativeProvider" |
//!                                     "registerVirtualModel" ] }
//! }
//! ```
//!
//! Result: `{ diagnostics, flagValues, modelRuntimeCalls }`.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use pi_agent::types::BoxFuture;
use pi_session::agent_session::services::{
    create_agent_session_services, CreateAgentSessionServicesOptions, ExtensionFlagSpec,
    ExtensionsRuntimeState, FlagValue, LoadedExtension, LoadExtensionsResult,
    PendingNativeProviderRegistration, PendingProviderRegistration,
    PendingVirtualModelRegistration, ServicesModelRuntime, ServicesResourceLoader,
};
use pi_session::agent_session::SessionSettingsStore;

/// Fake settings store (TS `SettingsManager.inMemory`).
struct ServicesHarnessSettings;

impl SessionSettingsStore for ServicesHarnessSettings {
    fn retry_settings(&self) -> pi_core::settings_manager::RetrySettings {
        pi_core::settings_manager::RetrySettings {
            enabled: true,
            max_retries: 3.0,
            base_delay_ms: 2000.0,
            max_agent_delay_ms: pi_session::agent_session::retry::DEFAULT_MAX_AGENT_RETRY_DELAY_MS,
        }
    }
}

/// Recording `ModelRuntime` fake (`throwOn` models per-case throws).
struct RecordingModelRuntime {
    calls: Mutex<Vec<Value>>,
    throw_on: Vec<String>,
}

impl ServicesModelRuntime for RecordingModelRuntime {
    fn register_provider(&self, name: &str, config: &Value) -> Result<(), String> {
        self.calls.lock().expect("calls").push(json!({
            "op": "registerProvider", "name": name, "config": config,
        }));
        if self.throw_on.iter().any(|op| op == "registerProvider") {
            return Err("provider boom".to_string());
        }
        Ok(())
    }
    fn register_native_provider(&self, provider: &Value) -> Result<(), String> {
        self.calls
            .lock()
            .expect("calls")
            .push(json!({ "op": "registerNativeProvider", "provider": provider }));
        if self.throw_on.iter().any(|op| op == "registerNativeProvider") {
            return Err("native provider boom".to_string());
        }
        Ok(())
    }
    fn register_virtual_model(&self, definition: &Value) -> Result<(), String> {
        self.calls
            .lock()
            .expect("calls")
            .push(json!({ "op": "registerVirtualModel", "definition": definition }));
        if self.throw_on.iter().any(|op| op == "registerVirtualModel") {
            return Err("virtual model boom".to_string());
        }
        Ok(())
    }
    fn refresh<'a>(&'a self, allow_network: bool) -> BoxFuture<'a, ()> {
        self.calls
            .lock()
            .expect("calls")
            .push(json!({ "op": "refresh", "allowNetwork": allow_network }));
        Box::pin(async {})
    }
}

/// Fake `ResourceLoader` mirroring the TS inline extension factories.
struct FakeResourceLoader {
    result: LoadExtensionsResult,
}

impl ServicesResourceLoader for FakeResourceLoader {
    fn get_extensions(&self) -> LoadExtensionsResult {
        LoadExtensionsResult {
            extensions: self.result.extensions.clone(),
            runtime: Arc::clone(&self.result.runtime),
        }
    }
}

pub fn run_agent_session_services_harness(args: &Value, process_cwd: &str) -> Result<Value, String> {
    // Validation order mirrors the TS entry exactly.
    let options = args.get("options").and_then(Value::as_object).ok_or_else(|| "missing options".to_string())?;
    let cwd = match options.get("cwd") {
        Some(Value::String(cwd)) => cwd.clone(),
        _ => return Err("missing cwd".to_string()),
    };
    let agent_dir = match options.get("agentDir") {
        None | Some(Value::Null) => None,
        Some(Value::String(dir)) => Some(dir.clone()),
        _ => return Err("invalid agentDir".to_string()),
    };
    let mut flag_values: Vec<(String, FlagValue)> = Vec::new();
    if let Some(pairs) = options.get("extensionFlagValues") {
        let pairs = pairs
            .as_array()
            .ok_or_else(|| "invalid extensionFlagValues".to_string())?;
        for pair in pairs {
            let pair = pair
                .as_array()
                .filter(|p| p.len() == 2)
                .ok_or_else(|| "invalid extensionFlagValues".to_string())?;
            let name = pair[0]
                .as_str()
                .ok_or_else(|| "invalid extensionFlagValues".to_string())?
                .to_string();
            let value = match &pair[1] {
                Value::Bool(b) => FlagValue::Bool(*b),
                Value::String(s) => FlagValue::Str(s.clone()),
                _ => return Err("invalid extensionFlagValues".to_string()),
            };
            // Mirror the TS harness's `new Map(flagPairs)`: duplicate
            // names collapse (first-insertion order, last value wins).
            match flag_values.iter_mut().find(|(n, _)| *n == name) {
                Some(slot) => slot.1 = value,
                None => flag_values.push((name, value)),
            }
        }
    }
    let mut extensions: Vec<LoadedExtension> = Vec::new();
    let mut pending_providers: Vec<PendingProviderRegistration> = Vec::new();
    let mut pending_native: Vec<PendingNativeProviderRegistration> = Vec::new();
    let mut pending_virtual: Vec<PendingVirtualModelRegistration> = Vec::new();
    if let Some(list) = args.get("extensions") {
        let list = list.as_array().ok_or_else(|| "invalid extensions".to_string())?;
        for ext in list {
            let ext = ext.as_object().ok_or_else(|| "invalid extension".to_string())?;
            let name = ext
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "invalid extension".to_string())?
                .to_string();
            let extension_path = format!("<inline:{}>", name);
            let mut flags: Vec<ExtensionFlagSpec> = Vec::new();
            if let Some(flag_list) = ext.get("flags") {
                let flag_list = flag_list.as_array().ok_or_else(|| "invalid flags".to_string())?;
                for flag in flag_list {
                    let flag = flag.as_object().ok_or_else(|| "invalid flag".to_string())?;
                    let flag_name = flag
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "invalid flag".to_string())?;
                    let flag_type = flag.get("type").and_then(Value::as_str).unwrap_or("");
                    if !matches!(flag_type, "boolean" | "string") {
                        return Err("invalid flag".to_string());
                    }
                    flags.push(ExtensionFlagSpec {
                        name: flag_name.to_string(),
                        flag_type: flag_type.to_string(),
                    });
                }
            }
            if let Some(providers) = ext.get("providers") {
                let providers = providers.as_array().ok_or_else(|| "invalid providers".to_string())?;
                for provider in providers {
                    let provider = provider
                        .as_object()
                        .ok_or_else(|| "invalid providers".to_string())?;
                    let provider_name = provider
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "invalid providers".to_string())?;
                    pending_providers.push(PendingProviderRegistration {
                        name: provider_name.to_string(),
                        config: provider.get("config").cloned().unwrap_or(json!({})),
                        extension_path: extension_path.clone(),
                    });
                }
            }
            if let Some(native) = ext.get("nativeProviders") {
                let native = native.as_array().ok_or_else(|| "invalid nativeProviders".to_string())?;
                for provider in native {
                    if !provider.is_object() {
                        return Err("invalid nativeProviders".to_string());
                    }
                    pending_native.push(PendingNativeProviderRegistration {
                        provider: provider.clone(),
                        extension_path: extension_path.clone(),
                    });
                }
            }
            if let Some(virtual_models) = ext.get("virtualModels") {
                let virtual_models = virtual_models
                    .as_array()
                    .ok_or_else(|| "invalid virtualModels".to_string())?;
                for definition in virtual_models {
                    if !definition.is_object() {
                        return Err("invalid virtualModels".to_string());
                    }
                    pending_virtual.push(PendingVirtualModelRegistration {
                        definition: definition.clone(),
                        extension_path: extension_path.clone(),
                    });
                }
            }
            extensions.push(LoadedExtension { name, flags });
        }
    }
    let mut throw_on: Vec<String> = Vec::new();
    if let Some(model_runtime) = args.get("modelRuntime") {
        let model_runtime = model_runtime
            .as_object()
            .ok_or_else(|| "invalid modelRuntime".to_string())?;
        if let Some(list) = model_runtime.get("throwOn") {
            let list = list.as_array().ok_or_else(|| "invalid modelRuntime".to_string())?;
            for op in list {
                let op = op
                    .as_str()
                    .ok_or_else(|| "invalid modelRuntime".to_string())?;
                throw_on.push(op.to_string());
            }
        }
    }

    // Mirror the TS harness: base = <case cwd>/pi-parity-assv is created
    // (rm + mkdir, idempotent) after validation and used as the default
    // agentDir. The empty dir stays in the case's filesystem snapshot on
    // both sides.
    let base = std::path::Path::new(process_cwd).join("pi-parity-assv");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).map_err(|e| format!("mkdir failed: {}", e))?;
    let agent_dir = agent_dir.unwrap_or_else(|| base.to_string_lossy().to_string());

    let runtime_state = Arc::new(Mutex::new(ExtensionsRuntimeState {
        flag_values: Vec::new(),
        pending_provider_registrations: pending_providers,
        pending_native_provider_registrations: pending_native,
        pending_virtual_model_registrations: pending_virtual,
    }));
    let resource_loader = Arc::new(FakeResourceLoader {
        result: LoadExtensionsResult {
            extensions,
            runtime: Arc::clone(&runtime_state),
        },
    });
    let model_runtime = Arc::new(RecordingModelRuntime {
        calls: Mutex::new(Vec::new()),
        throw_on,
    });

    pi_agent::run_current_thread(async move {
        let services = create_agent_session_services(CreateAgentSessionServicesOptions {
            cwd,
            agent_dir: Some(agent_dir),
            model_runtime: Arc::clone(&model_runtime) as Arc<dyn ServicesModelRuntime>,
            settings_manager: Arc::new(ServicesHarnessSettings) as Arc<dyn SessionSettingsStore>,
            extension_flag_values: flag_values,
            resource_loader,
        })
        .await?;
        let flag_values = services
            .resource_loader
            .get_extensions()
            .runtime
            .lock()
            .expect("extensions runtime")
            .flag_values
            .clone();
        let diagnostics = services
            .diagnostics
            .iter()
            .map(|d| json!({ "type": d.type_, "message": d.message }))
            .collect::<Vec<Value>>();
        Ok(json!({
            "result": {
                "diagnostics": diagnostics,
                "flagValues": flag_values,
                "modelRuntimeCalls": model_runtime.calls.lock().expect("calls").clone(),
            }
        }))
    })
}
