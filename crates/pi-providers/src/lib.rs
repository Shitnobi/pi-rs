//! Phase 3 provider/model layer: faithful Rust port of the TS
//! `packages/coding-agent/src/core/{provider,model,auth}-*.ts` layer.
//! See docs/plans/phase3-providers.md for the port plan.
//!
//! Module ownership (parallel port waves — do not edit modules you don't own):
//! - foundation wave: pi_ai, config_value, http_config, auth_guidance,
//!   runtime_credentials, models_store, virtual_models
//! - config/resolution wave: model_config, model_resolver, provider_attribution
//! - auth/composer wave: auth_storage, provider_composer, remote_catalog
//! - facade wave (later): model_runtime, model_registry

pub mod error;
pub mod pi_ai;
pub mod auth_guidance;
pub mod auth_storage;
pub mod config_value;
pub mod http_config;
pub mod model_config;
pub mod model_registry;
pub mod model_resolver;
pub mod model_runtime;
pub mod models_store;
pub mod provider_attribution;
pub mod provider_composer;
pub mod remote_catalog;
pub mod runtime_credentials;
pub mod virtual_models;
