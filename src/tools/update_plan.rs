use serde::Deserialize;
use serde_json::Value;

use super::{
    Tool, ToolCapabilities, ToolContext, ToolEffect, ToolError, ToolErrorCode, ToolExecutionMode,
    ToolOutput, ToolPermissionSpec, ToolSpec,
};
use crate::plan::{self, PlanItemPatch, PlanStatus};

pub struct UpdatePlan;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdatePlanArgs {
    #[serde(default)]
    explanation: Option<String>,
    #[serde(default)]
    items: Vec<PlanPatchArgs>,
    #[serde(default)]
    clear: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanPatchArgs {
    id: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    status: Option<PlanStatus>,
    #[serde(default)]
    remove: bool,
}

impl Tool for UpdatePlan {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "update_plan".into(),
            description: "Incrementally update the structured plan by stable item id. Existing items only need changed fields; new items require text and default to pending. Set remove=true to delete one item or clear=true to clear the plan. The server owns revision. Keep at most one item in_progress.".into(),
            schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "explanation": {
                        "type": "string",
                        "maxLength": plan::MAX_PLAN_EXPLANATION_CHARS
                    },
                    "items": {
                        "type": "array",
                        "maxItems": plan::MAX_PLAN_ITEMS,
                        "items": {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "id": {
                                    "type": "string",
                                    "minLength": 1,
                                    "maxLength": plan::MAX_PLAN_ID_CHARS
                                },
                                "text": {
                                    "type": "string",
                                    "minLength": 1,
                                    "maxLength": plan::MAX_PLAN_TEXT_CHARS
                                },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"]
                                },
                                "remove": {
                                    "type": "boolean",
                                    "description": "Delete this existing item; do not combine with text or status"
                                }
                            },
                            "required": ["id"]
                        }
                    },
                    "clear": {
                        "type": "boolean",
                        "description": "Clear the complete plan; do not combine with items or explanation"
                    }
                }
            }),
            capabilities: ToolCapabilities {
                read_only: true,
                destructive: false,
                execution_mode: ToolExecutionMode::Sequential,
                supports_background: false,
            },
            permission: ToolPermissionSpec::default(),
        }
    }

    fn execute(&self, args: &Value, ctx: &mut ToolContext<'_>) -> Result<ToolOutput, ToolError> {
        let args: UpdatePlanArgs = serde_json::from_value(args.clone()).map_err(|error| {
            ToolError::invalid_arguments(format!("invalid update_plan arguments: {error}"))
        })?;
        let patches = args
            .items
            .into_iter()
            .map(|patch| PlanItemPatch {
                id: patch.id,
                text: patch.text,
                status: patch.status,
                remove: patch.remove,
            })
            .collect();
        let snapshot = plan::patch_plan(ctx.current_plan(), patches, args.explanation, args.clear)
            .map_err(|error| {
                let code = match error.kind {
                    plan::PlanErrorKind::Invalid => ToolErrorCode::InvalidArguments,
                    plan::PlanErrorKind::Conflict => ToolErrorCode::Conflict,
                };
                ToolError::new(code, error.message)
            })?;
        let counts = snapshot.counts();
        let model_text = serde_json::json!({
            "revision": snapshot.revision,
            "pending": counts.pending,
            "in_progress": counts.in_progress,
            "completed": counts.completed,
        })
        .to_string();
        let ui_summary = Some(format!(
            "计划 #{}: {} 待处理，{} 进行中，{} 已完成",
            snapshot.revision, counts.pending, counts.in_progress, counts.completed
        ));
        ctx.record_effect(ToolEffect::PlanUpdated(snapshot));
        Ok(ToolOutput {
            model_text,
            images: Vec::new(),
            ui_summary,
            details: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::plan::{PlanSnapshot, PlanStatus};
    use crate::workspace::Workspace;

    fn execute(
        args: Value,
        current_plan: PlanSnapshot,
    ) -> (Result<ToolOutput, ToolError>, Vec<ToolEffect>) {
        let workspace = Workspace::new(std::env::temp_dir());
        let cancel = AtomicBool::new(false);
        let mut progress = |_| {};
        let mut context = ToolContext {
            workspace: &workspace,
            cancel: &cancel,
            session_id: "test",
            current_plan,
            progress: &mut progress,
            effects: Vec::new(),
        };
        let result = UpdatePlan.execute(&args, &mut context);
        let effects = context.take_effects();
        (result, effects)
    }

    #[test]
    fn emits_an_explicit_plan_effect() {
        let (result, effects) = execute(
            serde_json::json!({
                "explanation": "Starting",
                "items": [{"id": "inspect", "text": "Inspect code", "status": "in_progress"}]
            }),
            PlanSnapshot::default(),
        );
        assert!(result.is_ok());
        assert_eq!(effects.len(), 1);
        let ToolEffect::PlanUpdated(snapshot) = &effects[0];
        assert_eq!(snapshot.revision, 1);
        assert_eq!(snapshot.items[0].status, PlanStatus::InProgress);
    }

    #[test]
    fn patches_existing_items_without_resending_the_plan() {
        let (result, effects) = execute(
            serde_json::json!({"items": [{"id": "inspect", "status": "completed"}]}),
            PlanSnapshot {
                revision: 2,
                items: vec![crate::plan::PlanItem {
                    id: "inspect".into(),
                    text: "Inspect code".into(),
                    status: PlanStatus::InProgress,
                }],
                explanation: None,
            },
        );
        assert!(result.is_ok());
        let ToolEffect::PlanUpdated(snapshot) = &effects[0];
        assert_eq!(snapshot.revision, 3);
        assert_eq!(snapshot.items[0].status, PlanStatus::Completed);
    }

    #[test]
    fn clear_is_exclusive_and_emits_an_empty_snapshot() {
        let current = PlanSnapshot {
            revision: 3,
            items: vec![crate::plan::PlanItem {
                id: "inspect".into(),
                text: "Inspect code".into(),
                status: PlanStatus::InProgress,
            }],
            explanation: Some("active".into()),
        };
        let (result, effects) = execute(serde_json::json!({"clear": true}), current.clone());
        assert!(result.is_ok());
        let ToolEffect::PlanUpdated(snapshot) = &effects[0];
        assert_eq!(snapshot.revision, 4);
        assert!(snapshot.items.is_empty());
        assert_eq!(snapshot.explanation, None);

        let (result, effects) = execute(
            serde_json::json!({"clear": true, "explanation": "ambiguous"}),
            current,
        );
        assert_eq!(result.unwrap_err().code, ToolErrorCode::InvalidArguments);
        assert!(effects.is_empty());
    }
}
