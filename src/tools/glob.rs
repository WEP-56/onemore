//! Read-only, gitignore-aware file pattern discovery.

use globset::{Glob as GlobPattern, GlobSetBuilder};
use serde_json::{json, Value};

use super::workspace_walk::{relative_display, walk};
use super::{
    optional_u64, Tool, ToolCapabilities, ToolContext, ToolError, ToolErrorCode, ToolOutput,
    ToolPermissionSpec, ToolSpec,
};

const DEFAULT_LIMIT: u64 = 100;
const MAX_LIMIT: u64 = 1_000;

pub struct Glob;

impl Tool for Glob {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "glob".into(),
            description: "Find workspace files by glob pattern. Respects .gitignore, does not follow directory symlinks, and skips .git, target, node_modules, and common generated directories. Results are path-sorted and bounded.".into(),
            schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "pattern": { "type": "string", "minLength": 1, "description": "Glob pattern, for example **/*.rs or src/**/*.ts" },
                    "path": { "type": "string", "description": "Directory to search, default workspace root" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 1000, "description": "Maximum paths returned, default 100" }
                },
                "required": ["pattern"]
            }),
            capabilities: ToolCapabilities::READ_ONLY,
            permission: ToolPermissionSpec::paths(&["path"]),
        }
    }

    fn execute(&self, args: &Value, ctx: &mut ToolContext<'_>) -> Result<ToolOutput, ToolError> {
        let root = resolve_root(args, ctx)?;
        let pattern = super::require_str(args, "pattern")?;
        let limit = optional_u64(args, "limit")?
            .unwrap_or(DEFAULT_LIMIT)
            .clamp(1, MAX_LIMIT) as usize;
        let mut builder = GlobSetBuilder::new();
        builder.add(GlobPattern::new(pattern).map_err(|error| {
            ToolError::invalid_arguments(format!("invalid glob pattern: {error}"))
        })?);
        let matcher = builder.build().map_err(|error| {
            ToolError::invalid_arguments(format!("invalid glob pattern: {error}"))
        })?;

        let mut matches = Vec::new();
        let mut truncated = false;
        let mut scanned_files = 0usize;
        for entry in walk(&root) {
            if ctx.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(ToolError::new(ToolErrorCode::Aborted, "glob was cancelled"));
            }
            let entry = entry.map_err(|error| ToolError::io(error.to_string()))?;
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            scanned_files += 1;
            let relative = relative_display(&root, entry.path());
            if !matcher.is_match(&relative) {
                continue;
            }
            if matches.len() == limit {
                truncated = true;
                break;
            }
            matches.push(ctx.workspace.display_model_path(entry.path()));
        }

        let mut model_text = if matches.is_empty() {
            "No files found.".to_string()
        } else {
            matches.join("\n")
        };
        if truncated {
            model_text.push_str(&format!(
                "\n[truncated after {limit} paths; narrow pattern or path]"
            ));
        }
        Ok(ToolOutput {
            model_text,
            ui_summary: Some(format!("glob found {} path(s)", matches.len())),
            details: Some(json!({
                "path": ctx.workspace.display_model_path(&root),
                "pattern": pattern,
                "matches": matches,
                "scanned_files": scanned_files,
                "truncated": truncated,
                "limit": limit,
            })),
        })
    }
}

fn resolve_root(args: &Value, ctx: &ToolContext<'_>) -> Result<std::path::PathBuf, ToolError> {
    let given = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let root = ctx.workspace.resolve(given);
    if root.is_dir() {
        Ok(root)
    } else {
        Err(ToolError::new(
            ToolErrorCode::NotDirectory,
            format!("{} is not a directory", root.display()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;
    use std::path::Path;
    use std::sync::atomic::AtomicBool;

    fn temp_workspace(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "onemore-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn run(root: &Path, args: Value) -> Result<ToolOutput, ToolError> {
        let workspace = Workspace::new(root.to_path_buf());
        let cancel = AtomicBool::new(false);
        Glob.execute(
            &args,
            &mut ToolContext {
                workspace: &workspace,
                cancel: &cancel,
                session_id: "test",
                current_plan: crate::plan::PlanSnapshot::default(),
                progress: &mut |_| {},
                effects: Vec::new(),
            },
        )
    }

    #[test]
    fn matches_files_and_skips_generated_directories() {
        let root = temp_workspace("glob");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("src").join("main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("target").join("generated.rs"), "").unwrap();

        let output = run(&root, json!({ "pattern": "**/*.rs" })).unwrap();
        assert!(output.model_text.contains("src"));
        assert!(!output.model_text.contains('\\'));
        assert!(!output.model_text.contains("generated.rs"));
        assert_eq!(output.details.unwrap()["truncated"], false);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reports_result_truncation() {
        let root = temp_workspace("glob-limit");
        std::fs::write(root.join("a.txt"), "").unwrap();
        std::fs::write(root.join("b.txt"), "").unwrap();

        let output = run(&root, json!({ "pattern": "*.txt", "limit": 1 })).unwrap();
        assert!(output.model_text.contains("truncated"));
        assert_eq!(
            output.details.unwrap()["matches"].as_array().unwrap().len(),
            1
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
