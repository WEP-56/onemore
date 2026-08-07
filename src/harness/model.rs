use anyhow::{bail, Result};

use crate::config::{
    ActiveModelSelection, ModelCatalogEntry, ProviderCatalogEntry, ProviderSettings,
    ReasoningEffortPolicy,
};

use super::ModelRegistry;

/// A programmatic registry for one already-resolved provider/model selection.
/// It is the minimal model surface needed by an embedded agent.
pub struct FixedModelRegistry {
    settings: ProviderSettings,
}

impl FixedModelRegistry {
    pub fn new(settings: ProviderSettings) -> Self {
        FixedModelRegistry { settings }
    }

    fn selection(&self) -> ActiveModelSelection {
        ActiveModelSelection {
            provider: self.settings.name.clone(),
            model: self.settings.model.clone(),
            effort: self.settings.selected_effort.clone(),
        }
    }
}

impl ModelRegistry for FixedModelRegistry {
    fn initial_selection(&self) -> Result<ActiveModelSelection> {
        Ok(self.selection())
    }

    fn default_selection(&self, provider: &str) -> Result<ActiveModelSelection> {
        if provider != self.settings.name {
            bail!("fixed model registry 没有 provider {:?}", provider);
        }
        Ok(self.selection())
    }

    fn resolve_selection(&self, selection: &ActiveModelSelection) -> Result<ProviderSettings> {
        self.validate_selection(selection)?;
        Ok(self.settings.clone())
    }

    fn validate_selection(&self, selection: &ActiveModelSelection) -> Result<()> {
        let expected = self.selection();
        if selection != &expected {
            bail!(
                "fixed model registry 只支持 {}/{}/{}",
                expected.provider,
                expected.model,
                expected.effort
            );
        }
        Ok(())
    }

    fn model_default_effort(&self, provider: &str, model: &str) -> Result<String> {
        if provider != self.settings.name || model != self.settings.model {
            bail!("fixed model registry 没有模型 {}/{}", provider, model);
        }
        Ok(self.settings.selected_effort.clone())
    }

    fn provider_catalog(&self) -> Vec<ProviderCatalogEntry> {
        let sends_effort = matches!(
            self.settings.reasoning_effort,
            ReasoningEffortPolicy::Send(_)
        );
        vec![ProviderCatalogEntry {
            name: self.settings.name.clone(),
            default_model: self.settings.model.clone(),
            models: vec![ModelCatalogEntry {
                id: self.settings.model.clone(),
                context_window: self.settings.context_window,
                max_tokens: self.settings.max_tokens,
                efforts: if sends_effort {
                    vec![self.settings.selected_effort.clone()]
                } else {
                    Vec::new()
                },
                default_effort: self.settings.selected_effort.clone(),
                sends_effort,
            }],
        }]
    }
}
