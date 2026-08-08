//! Host-owned state interfaces used by the agent runtime.
//!
//! The default CLI adapters persist to SQLite and JSON. Embedders can inject
//! their own implementations or use the in-memory implementations here without
//! creating Onemore data directories.

use std::collections::BTreeMap;

use anyhow::{bail, Result};

use crate::config::{ActiveModelSelection, ProviderCatalogEntry, ProviderSettings};
use crate::message::Usage;
use crate::session::{SessionEntry, SessionEntryPayload, SessionList, SessionListScope};

mod memory;
mod model;

pub use memory::{MemoryModelPreferences, MemorySessionBackend};
pub use model::FixedModelRegistry;

/// Resolves model selections independently of any file configuration format.
pub trait ModelRegistry: Send {
    fn initial_selection(&self) -> Result<ActiveModelSelection>;
    fn default_selection(&self, provider: &str) -> Result<ActiveModelSelection>;
    fn resolve_selection(&self, selection: &ActiveModelSelection) -> Result<ProviderSettings>;
    fn validate_selection(&self, selection: &ActiveModelSelection) -> Result<()>;
    fn model_default_effort(&self, provider: &str, model: &str) -> Result<String>;
    fn provider_catalog(&self) -> Vec<ProviderCatalogEntry>;
}

/// Append-only fact storage plus optional session-management capabilities.
///
/// A minimal host only needs to implement [`SessionBackend::current_id`] and
/// [`SessionBackend::append_payloads`]. Unsupported management commands return
/// explicit errors through the defaults below.
pub trait SessionBackend: Send {
    fn current_id(&self) -> &str;

    /// Atomically append one validated fact batch and return its assigned IDs.
    /// Returning an error must leave durable and in-memory state unchanged.
    fn append_payloads(
        &mut self,
        payloads: Vec<SessionEntryPayload>,
        usage: Usage,
    ) -> Result<Vec<SessionEntry>>;

    fn clear(&mut self) -> Result<()> {
        bail!("当前 session backend 不支持清空")
    }

    fn list(&self, _scope: SessionListScope) -> Result<SessionList> {
        bail!("当前 session backend 不支持列出会话")
    }

    fn load(&mut self, _requested_id: &str) -> Result<(Vec<SessionEntry>, Usage)> {
        bail!("当前 session backend 不支持恢复会话")
    }
}

/// Workspace-scoped model reasoning preferences.
pub trait ModelPreferences: Send {
    fn effort(&self, provider: &str, model: &str) -> Option<&str>;

    fn reasoning_efforts(&self) -> BTreeMap<String, BTreeMap<String, String>>;

    fn set_effort(
        &mut self,
        provider: &str,
        model: &str,
        effort: &str,
        default_effort: &str,
    ) -> Result<()>;
}
