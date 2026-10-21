//! `createAgentSessionServices` (TS `core/agent-session-services.ts`, 233
//! LOC): cwd-bound runtime services creation plus extension flag
//! validation. The real `ModelRuntime`, `SettingsManager.create`, and
//! `DefaultResourceLoader` constructions are Phase 4+; this port takes
//! them as injected traits (the parity harness supplies recording fakes,
//! the TS harness injects equivalents through `options.modelRuntime` /
//! `options.settingsManager` / inline extension factories).

use std::sync::{Arc, Mutex};

use pi_agent::types::BoxFuture;

use super::SessionSettingsStore;

/// TS `AgentSessionRuntimeDiagnostic` (`{ type, message }`; `type_` avoids
/// the Rust keyword — the harness serializes it as `type`).
#[derive(Clone, Debug, PartialEq)]
pub struct AgentSessionRuntimeDiagnostic {
    pub type_: String,
    pub message: String,
}

impl AgentSessionRuntimeDiagnostic {
    fn error(message: String) -> AgentSessionRuntimeDiagnostic {
        AgentSessionRuntimeDiagnostic { type_: "error".to_string(), message }
    }
}

/// TS `extensionFlagValues` entry (`boolean | string`).
#[derive(Clone, Debug)]
pub enum FlagValue {
    Bool(bool),
    Str(String),
}

/// TS `ExtensionFlag` (the slice `applyExtensionFlagValues` reads).
#[derive(Clone, Debug)]
pub struct ExtensionFlagSpec {
    pub name: String,
    /// `"boolean" | `"string"`.
    pub flag_type: String,
}

/// TS `LoadExtensionsResult.extensions[i]` (name + ordered `flags` Map).
#[derive(Clone, Debug)]
pub struct LoadedExtension {
    pub name: String,
    pub flags: Vec<ExtensionFlagSpec>,
}

/// TS `LoadExtensionsResult.runtime.pendingProviderRegistrations[i]`.
#[derive(Clone, Debug)]
pub struct PendingProviderRegistration {
    pub name: String,
    pub config: serde_json::Value,
    pub extension_path: String,
}

/// TS `pendingNativeProviderRegistrations[i]`.
#[derive(Clone, Debug)]
pub struct PendingNativeProviderRegistration {
    pub provider: serde_json::Value,
    pub extension_path: String,
}

/// TS `pendingVirtualModelRegistrations[i]`.
#[derive(Clone, Debug)]
pub struct PendingVirtualModelRegistration {
    pub definition: serde_json::Value,
    pub extension_path: String,
}

/// TS `LoadExtensionsResult.runtime` — the mutable shared state
/// `createAgentSessionServices` drains and `applyExtensionFlagValues`
/// writes (`flagValues` Map).
#[derive(Default)]
pub struct ExtensionsRuntimeState {
    pub flag_values: Vec<(String, serde_json::Value)>,
    pub pending_provider_registrations: Vec<PendingProviderRegistration>,
    pub pending_native_provider_registrations: Vec<PendingNativeProviderRegistration>,
    pub pending_virtual_model_registrations: Vec<PendingVirtualModelRegistration>,
}

impl ExtensionsRuntimeState {
    /// TS `flagValues.set(name, value)` (Map dedupe: last write wins,
    /// original insertion position retained).
    pub fn set_flag_value(&mut self, name: &str, value: serde_json::Value) {
        if let Some(slot) = self.flag_values.iter_mut().find(|(k, _)| k == name) {
            slot.1 = value;
        } else {
            self.flag_values.push((name.to_string(), value));
        }
    }
}

/// TS `LoadExtensionsResult`.
#[derive(Clone)]
pub struct LoadExtensionsResult {
    pub extensions: Vec<LoadedExtension>,
    pub runtime: Arc<Mutex<ExtensionsRuntimeState>>,
}

/// TS `ResourceLoader` seam (the `getExtensions`/`reload` slice).
pub trait ServicesResourceLoader: Send + Sync {
    fn get_extensions(&self) -> LoadExtensionsResult;
    /// TS `reload(options)` — the real `DefaultResourceLoader.reload` scans
    /// the filesystem (Phase 4+); injected loaders default to a no-op.
    fn reload<'a>(&'a self) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}

/// TS `ModelRuntime` seam (the registration + refresh slice
/// `createAgentSessionServices` consumes).
pub trait ServicesModelRuntime: Send + Sync {
    /// TS `registerProvider(name, config)` — `Err` models a throw.
    fn register_provider(&self, name: &str, config: &serde_json::Value) -> Result<(), String>;
    /// TS `registerNativeProvider(provider)` — `Err` models a throw.
    fn register_native_provider(&self, provider: &serde_json::Value) -> Result<(), String>;
    /// TS `registerVirtualModel(definition)` — `Err` models a throw.
    fn register_virtual_model(&self, definition: &serde_json::Value) -> Result<(), String>;
    /// TS `refresh({ allowNetwork })`.
    fn refresh<'a>(&'a self, allow_network: bool) -> BoxFuture<'a, ()>;
}

/// TS `CreateAgentSessionServicesOptions`. The TS defaults
/// (`ModelRuntime.create`, `SettingsManager.create`, `new
/// DefaultResourceLoader`) are Phase 4+; callers inject all three.
pub struct CreateAgentSessionServicesOptions {
    pub cwd: String,
    pub agent_dir: Option<String>,
    pub model_runtime: Arc<dyn ServicesModelRuntime>,
    pub settings_manager: Arc<dyn SessionSettingsStore>,
    /// TS `extensionFlagValues` (ordered).
    pub extension_flag_values: Vec<(String, FlagValue)>,
    pub resource_loader: Arc<dyn ServicesResourceLoader>,
}

/// TS `AgentSessionServices`.
pub struct AgentSessionServices {
    pub cwd: String,
    pub agent_dir: String,
    pub model_runtime: Arc<dyn ServicesModelRuntime>,
    pub settings_manager: Arc<dyn SessionSettingsStore>,
    pub resource_loader: Arc<dyn ServicesResourceLoader>,
    pub diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
}

/// TS `applyExtensionFlagValues`.
fn apply_extension_flag_values(
    resource_loader: &dyn ServicesResourceLoader,
    extension_flag_values: &[(String, FlagValue)],
) -> Vec<AgentSessionRuntimeDiagnostic> {
    let mut diagnostics: Vec<AgentSessionRuntimeDiagnostic> = Vec::new();
    let extensions_result = resource_loader.get_extensions();
    // TS `registeredFlags` Map (name → { type }).
    let mut registered_flags: Vec<(String, String)> = Vec::new();
    for extension in &extensions_result.extensions {
        for flag in &extension.flags {
            if let Some(slot) = registered_flags.iter_mut().find(|(k, _)| k == &flag.name) {
                slot.1 = flag.flag_type.clone();
            } else {
                registered_flags.push((flag.name.clone(), flag.flag_type.clone()));
            }
        }
    }

    let mut unknown_flags: Vec<String> = Vec::new();
    for (name, value) in extension_flag_values {
        let flag_type = registered_flags.iter().find(|(k, _)| k == name).map(|(_, t)| t.clone());
        let Some(flag_type) = flag_type else {
            unknown_flags.push(name.clone());
            continue;
        };
        let mut runtime = extensions_result.runtime.lock().expect("extensions runtime");
        if flag_type == "boolean" {
            runtime.set_flag_value(name, serde_json::json!(true));
            continue;
        }
        if let FlagValue::Str(text) = value {
            runtime.set_flag_value(name, serde_json::json!(text));
            continue;
        }
        diagnostics.push(AgentSessionRuntimeDiagnostic::error(format!(
            "Extension flag \"--{}\" requires a value",
            name
        )));
    }

    if !unknown_flags.is_empty() {
        diagnostics.push(AgentSessionRuntimeDiagnostic::error(format!(
            "Unknown option{}: {}",
            if unknown_flags.len() == 1 { "" } else { "s" },
            unknown_flags
                .iter()
                .map(|name| format!("--{}", name))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    diagnostics
}

/// TS `createAgentSessionServices`.
pub async fn create_agent_session_services(
    options: CreateAgentSessionServicesOptions,
) -> Result<AgentSessionServices, String> {
    let cwd = pi_core::session_manager::resolve_ambient(&options.cwd);
    let agent_dir = options
        .agent_dir
        .map(|d| pi_core::session_manager::resolve_ambient(&d))
        .unwrap_or_else(|| pi_core::session_manager::resolve_ambient("."));
    let model_runtime = options.model_runtime;
    let settings_manager = options.settings_manager;
    let resource_loader = options.resource_loader;
    resource_loader.reload().await;

    let mut diagnostics: Vec<AgentSessionRuntimeDiagnostic> = Vec::new();
    let extensions_result = resource_loader.get_extensions();

    let pending_providers = {
        let mut runtime = extensions_result.runtime.lock().expect("extensions runtime");
        std::mem::take(&mut runtime.pending_provider_registrations)
    };
    for registration in pending_providers {
        if let Err(error) = model_runtime.register_provider(&registration.name, &registration.config) {
            diagnostics.push(AgentSessionRuntimeDiagnostic::error(format!(
                "Extension \"{}\" error: {}",
                registration.extension_path, error
            )));
        }
    }
    let pending_native = {
        let mut runtime = extensions_result.runtime.lock().expect("extensions runtime");
        std::mem::take(&mut runtime.pending_native_provider_registrations)
    };
    for registration in pending_native {
        if let Err(error) = model_runtime.register_native_provider(&registration.provider) {
            diagnostics.push(AgentSessionRuntimeDiagnostic::error(format!(
                "Extension \"{}\" error: {}",
                registration.extension_path, error
            )));
        }
    }
    let pending_virtual = {
        let mut runtime = extensions_result.runtime.lock().expect("extensions runtime");
        std::mem::take(&mut runtime.pending_virtual_model_registrations)
    };
    for registration in pending_virtual {
        if let Err(error) = model_runtime.register_virtual_model(&registration.definition) {
            diagnostics.push(AgentSessionRuntimeDiagnostic::error(format!(
                "Extension \"{}\" error: {}",
                registration.extension_path, error
            )));
        }
    }
    model_runtime.refresh(false).await;
    diagnostics.extend(apply_extension_flag_values(
        resource_loader.as_ref(),
        &options.extension_flag_values,
    ));

    Ok(AgentSessionServices {
        cwd,
        agent_dir,
        model_runtime,
        settings_manager,
        resource_loader,
        diagnostics,
    })
}
