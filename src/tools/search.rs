//! Read-only, bounded regular-expression search over workspace text files.

use regex::RegexBuilder;
use serde_json::{json, Value};

use super::workspace_walk::{relative_display, walk};
use super::{
    optional_u64, Tool, ToolCapabilities, ToolContext, ToolError, ToolErrorCode, ToolOutput,
    ToolPermissionSpec, ToolSpec,
};

const DEFAULT_LIMIT: u64 = 100;
const MAX_LIMIT: u64 = 1_000;
const MAX_SEARCH_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_LINE_CHARS: usize = 500;
const MAX_CONTEXT_LINES: u64 = 10;

pub struct Search;

impl Tool for Search {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "search".into(),
            description: "Search workspace text files with a line-oriented Rust regular expression. Respects .gitignore, skips generated directories, and returns bounded path:line matches. Use context for nearby lines; patterns do not span newlines.".into(),
            schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "pattern": { "type": "string", "minLength": 1, "description": "Rust regular expression to search for" },
                    "path": { "type": "string", "description": "Directory to search, default workspace root" },
                    "include": { "type": "string", "minLength": 1, "description": "Optional file glob filter, for example *.rs" },
                    "case_sensitive": { "type": "boolean", "description": "Whether matching is case-sensitive, default true" },
                    "context": { "type": "integer", "minimum": 0, "maximum": 10, "description": "Lines of context before and after each match, default 0" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 1000, "description": "Maximum matching lines returned, default 100" }
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
        let case_sensitive = args
            .get("case_sensitive")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let regex = RegexBuilder::new(pattern)
            .case_insensitive(!case_sensitive)
            .build()
            .map_err(|error| {
                ToolError::invalid_arguments(format!("invalid regular expression: {error}"))
            })?;
        let include = compile_include(args.get("include").and_then(Value::as_str))?;
        let limit = optional_u64(args, "limit")?
            .unwrap_or(DEFAULT_LIMIT)
            .clamp(1, MAX_LIMIT) as usize;
        let context_lines = optional_u64(args, "context")?
            .unwrap_or(0)
            .min(MAX_CONTEXT_LINES) as usize;

        let mut matches = Vec::new();
        let mut scanned_files = 0usize;
        let mut skipped_large_files = 0usize;
        let mut skipped_non_utf8_files = 0usize;
        let mut skipped_unreadable_files = 0usize;
        let mut truncated = false;

        'entries: for entry in walk(&root) {
            if ctx.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(ToolError::new(
                    ToolErrorCode::Aborted,
                    "search was cancelled",
                ));
            }
            let entry = entry.map_err(|error| ToolError::io(error.to_string()))?;
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let relative = relative_display(&root, entry.path());
            if !include
                .as_ref()
                .is_none_or(|matcher| matcher.is_match(&relative))
            {
                continue;
            }
            let metadata = match ctx.workspace.metadata(entry.path()) {
                Ok(metadata) => metadata,
                Err(_) => {
                    skipped_unreadable_files += 1;
                    continue;
                }
            };
            if metadata.len() > MAX_SEARCH_FILE_BYTES {
                skipped_large_files += 1;
                continue;
            }
            let content = match ctx.workspace.read_text(entry.path()) {
                Ok(content) => content,
                Err(error) if error.contains("not UTF-8") || error.contains("UTF-8") => {
                    skipped_non_utf8_files += 1;
                    continue;
                }
                Err(_) => {
                    skipped_unreadable_files += 1;
                    continue;
                }
            };
            scanned_files += 1;
            let lines = content.lines().collect::<Vec<_>>();
            for (index, raw_line) in lines.iter().enumerate() {
                if !regex.is_match(raw_line) {
                    continue;
                }
                if matches.len() == limit {
                    truncated = true;
                    break 'entries;
                }
                let before_start = index.saturating_sub(context_lines);
                let after_end = (index + context_lines + 1).min(lines.len());
                matches.push(json!({
                    "path": ctx.workspace.display_model_path(entry.path()),
                    "line": index + 1,
                    "text": truncate_line(raw_line),
                    "before": lines[before_start..index].iter().enumerate().map(|(offset, line)| json!({
                        "line": before_start + offset + 1,
                        "text": truncate_line(line),
                    })).collect::<Vec<_>>(),
                    "after": lines[index + 1..after_end].iter().enumerate().map(|(offset, line)| json!({
                        "line": index + offset + 2,
                        "text": truncate_line(line),
                    })).collect::<Vec<_>>(),
                }));
            }
        }

        let mut model_text = if matches.is_empty() {
            "No matches found.".to_string()
        } else {
            let separator = if context_lines == 0 { "\n" } else { "\n--\n" };
            matches
                .iter()
                .map(render_match)
                .collect::<Vec<_>>()
                .join(separator)
        };
        if truncated {
            model_text.push_str(&format!(
                "\n[truncated after {limit} matching lines; narrow pattern or path]"
            ));
        }
        Ok(ToolOutput {
            model_text,
            ui_summary: Some(format!("search found {} match(es)", matches.len())),
            details: Some(json!({
                "path": ctx.workspace.display_model_path(&root),
                "pattern": pattern,
                "include": args.get("include").and_then(Value::as_str),
                "case_sensitive": case_sensitive,
                "context": context_lines,
                "matches": matches,
                "scanned_files": scanned_files,
                "skipped_large_files": skipped_large_files,
                "skipped_non_utf8_files": skipped_non_utf8_files,
                "skipped_unreadable_files": skipped_unreadable_files,
                "truncated": truncated,
                "limit": limit,
            })),
        })
    }
}

fn render_match(item: &Value) -> String {
    let path = item["path"].as_str().unwrap_or_default();
    let mut rendered = Vec::new();
    for line in item["before"].as_array().into_iter().flatten() {
        rendered.push(format!(
            "{}-{}- {}",
            path,
            line["line"].as_u64().unwrap_or_default(),
            line["text"].as_str().unwrap_or_default(),
        ));
    }
    rendered.push(format!(
        "{}:{}: {}",
        path,
        item["line"].as_u64().unwrap_or_default(),
        item["text"].as_str().unwrap_or_default(),
    ));
    for line in item["after"].as_array().into_iter().flatten() {
        rendered.push(format!(
            "{}-{}- {}",
            path,
            line["line"].as_u64().unwrap_or_default(),
            line["text"].as_str().unwrap_or_default(),
        ));
    }
    rendered.join("\n")
}

fn compile_include(pattern: Option<&str>) -> Result<Option<globset::GlobSet>, ToolError> {
    let Some(pattern) = pattern.filter(|pattern| !pattern.is_empty()) else {
        return Ok(None);
    };
    let mut builder = globset::GlobSetBuilder::new();
    builder.add(
        globset::Glob::new(pattern).map_err(|error| {
            ToolError::invalid_arguments(format!("invalid include glob: {error}"))
        })?,
    );
    builder
        .build()
        .map(Some)
        .map_err(|error| ToolError::invalid_arguments(format!("invalid include glob: {error}")))
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

fn truncate_line(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_CHARS {
        return line.trim_end_matches('\r').to_string();
    }
    let prefix: String = line.chars().take(MAX_LINE_CHARS).collect();
    format!("{prefix}...[line truncated]")
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
        Search.execute(
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
    fn finds_regex_matches_and_honors_include() {
        let root = temp_workspace("search");
        std::fs::write(root.join("main.rs"), "fn alpha() {}\nlet beta = 1;\n").unwrap();
        std::fs::write(root.join("notes.txt"), "alpha\n").unwrap();

        let output = run(&root, json!({ "pattern": "alpha", "include": "*.rs" })).unwrap();
        assert!(output.model_text.contains("main.rs:1"));
        assert!(!output.model_text.contains('\\'));
        assert!(!output.model_text.contains("notes.txt"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reports_match_truncation() {
        let root = temp_workspace("search-limit");
        std::fs::write(root.join("sample.txt"), "match\nmatch\n").unwrap();
        let output = run(&root, json!({ "pattern": "match", "limit": 1 })).unwrap();
        assert!(output.model_text.contains("truncated"));
        assert_eq!(
            output.details.unwrap()["matches"].as_array().unwrap().len(),
            1
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn returns_bounded_context_around_each_match() {
        let root = temp_workspace("search-context");
        std::fs::write(root.join("sample.txt"), "zero\none\nMATCH\nthree\nfour\n").unwrap();
        let output = run(&root, json!({ "pattern": "MATCH", "context": 1 })).unwrap();
        assert!(output.model_text.contains("sample.txt-2- one"));
        assert!(output.model_text.contains("sample.txt:3: MATCH"));
        assert!(output.model_text.contains("sample.txt-4- three"));
        assert!(!output.model_text.contains("zero"));
        let details = output.details.unwrap();
        assert_eq!(details["context"], 1);
        assert_eq!(details["matches"][0]["before"][0]["line"], 2);
        assert_eq!(details["matches"][0]["after"][0]["line"], 4);
        let _ = std::fs::remove_dir_all(root);
    }
}
