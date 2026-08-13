//! Bounded read-only Git queries. Commands never go through a shell.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{
    Tool, ToolCapabilities, ToolContext, ToolError, ToolErrorCode, ToolOutput, ToolPermissionSpec,
    ToolSpec,
};

const GIT_TIMEOUT: Duration = Duration::from_secs(5);
const STATUS_MAX_BYTES: usize = 256 * 1024;
const DIFF_MAX_BYTES: usize = 128 * 1024;
const DIFF_SUMMARY_MAX_BYTES: usize = 128 * 1024;
const DIFF_MODEL_MAX_CHARS: usize = 20_000;
const DIFF_SUMMARY_MODEL_MAX_CHARS: usize = 6_000;
const DIFF_SUMMARY_ENTRY_LIMIT: usize = 200;
const CHANGED_PATH_LIMIT: usize = 100;
const DISABLED_HOOKS_PATH: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

pub struct RepoState;
pub struct GitDiff;

impl Tool for RepoState {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "repo_state".into(),
            description: "Inspect the current Git repository without changing it. Returns repository root, branch, HEAD, and a bounded porcelain summary of changed paths.".into(),
            schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "path": { "type": "string", "description": "Directory used to locate the repository, default workspace root" }
                },
                "required": []
            }),
            capabilities: ToolCapabilities::READ_ONLY,
            permission: ToolPermissionSpec::paths(&["path"]),
        }
    }

    fn execute(&self, args: &Value, ctx: &mut ToolContext<'_>) -> Result<ToolOutput, ToolError> {
        let cwd = resolve_directory(args, ctx)?;
        let Some(repo_root) = git_repo_root(&cwd, ctx)? else {
            return Ok(ToolOutput {
                model_text: "Not a Git repository.".into(),
                images: Vec::new(),
                ui_summary: Some("not a Git repository".into()),
                details: Some(json!({
                    "path": ctx.workspace.display_model_path(&cwd),
                    "is_git_repository": false,
                })),
            });
        };
        let branch = git_text(
            &repo_root,
            &["symbolic-ref", "--quiet", "--short", "HEAD"],
            ctx,
        )
        .ok()
        .flatten();
        let head = git_text(&repo_root, &["rev-parse", "--short", "HEAD"], ctx)?;
        let status = run_git(
            &repo_root,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
            ctx,
            STATUS_MAX_BYTES,
        )?;
        if !status.success() && !status.truncated {
            return Err(git_failure("inspect repository state", &status));
        }
        let parsed = parse_status(&status.stdout, status.truncated);
        let mut model_text = format!(
            "Repository: {}\nBranch: {}\nHEAD: {}\nChanged: {} (staged {}, unstaged {}, untracked {})",
            ctx.workspace.display_model_path(&repo_root),
            branch.as_deref().unwrap_or("(detached)"),
            head.as_deref().unwrap_or("(no commits)"),
            parsed.total,
            parsed.staged,
            parsed.unstaged,
            parsed.untracked,
        );
        for entry in &parsed.entries {
            model_text.push_str(&format!("\n{} {}", entry.status, entry.path));
        }
        if parsed.truncated {
            model_text.push_str("\n[changed-path list truncated]");
        }
        Ok(ToolOutput {
            model_text,
            images: Vec::new(),
            ui_summary: Some(format!("{} changed path(s)", parsed.total)),
            details: Some(json!({
                "path": ctx.workspace.display_model_path(&cwd),
                "is_git_repository": true,
                "repository_root": ctx.workspace.display_model_path(&repo_root),
                "branch": branch,
                "head": head,
                "changed_paths": parsed.entries.iter().map(|entry| json!({
                    "path": entry.path,
                    "status": entry.status,
                })).collect::<Vec<_>>(),
                "changed_path_count": parsed.total,
                "staged_count": parsed.staged,
                "unstaged_count": parsed.unstaged,
                "untracked_count": parsed.untracked,
                "truncated": parsed.truncated,
            })),
        })
    }
}

impl Tool for GitDiff {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "git_diff".into(),
            description: "Return a file summary followed by a bounded, color-free Git patch without changing the repository. path filters the diff to that directory. By default compares the worktree with the index; set staged for the index diff or base to compare a revision with the worktree.".into(),
            schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "path": { "type": "string", "description": "Directory to include in the diff, default workspace root" },
                    "base": { "type": "string", "minLength": 1, "description": "Optional revision to compare with the worktree" },
                    "staged": { "type": "boolean", "description": "Compare the index with HEAD instead of the worktree, default false" }
                },
                "required": []
            }),
            capabilities: ToolCapabilities::READ_ONLY,
            permission: ToolPermissionSpec::paths(&["path"]),
        }
    }

    fn execute(&self, args: &Value, ctx: &mut ToolContext<'_>) -> Result<ToolOutput, ToolError> {
        let cwd = resolve_directory(args, ctx)?;
        let Some(repo_root) = git_repo_root(&cwd, ctx)? else {
            return Ok(ToolOutput {
                model_text: "Not a Git repository.".into(),
                images: Vec::new(),
                ui_summary: Some("not a Git repository".into()),
                details: Some(
                    json!({ "path": ctx.workspace.display_model_path(&cwd), "is_git_repository": false }),
                ),
            });
        };
        let staged = args.get("staged").and_then(Value::as_bool).unwrap_or(false);
        let base = args.get("base").and_then(Value::as_str);
        if let Some(base) = base {
            if base.starts_with('-') || base.contains('\0') {
                return Err(ToolError::invalid_arguments(
                    "base must be a Git revision, not an option",
                ));
            }
        }
        if staged && base.is_some() {
            return Err(ToolError::invalid_arguments(
                "base and staged cannot be used together",
            ));
        }
        let pathspec = git_pathspec(&cwd, ctx)?;
        let summary_args = diff_arguments(staged, base, pathspec.as_deref(), true);
        let summary_refs = summary_args.iter().map(String::as_str).collect::<Vec<_>>();
        let summary_output = run_git(&repo_root, &summary_refs, ctx, DIFF_SUMMARY_MAX_BYTES)?;
        if !summary_output.success() && !summary_output.truncated {
            return Err(git_failure("summarize Git diff", &summary_output));
        }
        let summary = parse_numstat(&summary_output.stdout, summary_output.truncated);
        let (summary_text, summary_model_truncated) = render_diff_summary(&summary);
        let summary_truncated = summary.truncated || summary_model_truncated;

        let patch_args = diff_arguments(staged, base, pathspec.as_deref(), false);
        let patch_refs = patch_args.iter().map(String::as_str).collect::<Vec<_>>();
        let output = run_git(&repo_root, &patch_refs, ctx, DIFF_MAX_BYTES)?;
        if !output.success() && !output.truncated {
            return Err(git_failure("read Git diff", &output));
        }
        let diff = String::from_utf8_lossy(&output.stdout).into_owned();
        let patch_budget = DIFF_MODEL_MAX_CHARS
            .saturating_sub(summary_text.chars().count() + 9)
            .max(1_000);
        let (patch_text, text_truncated) = bound_diff(&diff, patch_budget);
        let patch_truncated = output.truncated || text_truncated;
        let truncated = summary_truncated || patch_truncated;
        let model_text = if patch_text.trim().is_empty() {
            "No changes.".into()
        } else {
            format!("{summary_text}\n\nPatch:\n{patch_text}")
        };
        Ok(ToolOutput {
            model_text,
            images: Vec::new(),
            ui_summary: Some(if truncated {
                "Git diff (truncated)".into()
            } else {
                "Git diff".into()
            }),
            details: Some(json!({
                "path": ctx.workspace.display_model_path(&cwd),
                "is_git_repository": true,
                "repository_root": ctx.workspace.display_model_path(&repo_root),
                "pathspec": pathspec,
                "base": base,
                "staged": staged,
                "changed_files": summary.entries.iter().map(|entry| json!({
                    "path": entry.path,
                    "old_path": entry.old_path,
                    "additions": entry.additions,
                    "deletions": entry.deletions,
                    "binary": entry.binary,
                })).collect::<Vec<_>>(),
                "changed_file_count": summary.total,
                "addition_count": summary.additions,
                "deletion_count": summary.deletions,
                "binary_file_count": summary.binary_files,
                "summary_truncated": summary_truncated,
                "returned_bytes": output.stdout.len(),
                "patch_truncated": patch_truncated,
                "truncated": truncated,
            })),
        })
    }
}

fn diff_arguments(
    staged: bool,
    base: Option<&str>,
    pathspec: Option<&str>,
    numstat: bool,
) -> Vec<String> {
    let mut args = vec![
        "diff".to_string(),
        "--no-ext-diff".to_string(),
        "--no-color".to_string(),
    ];
    if numstat {
        args.extend(["--numstat".to_string(), "-z".to_string()]);
    } else {
        args.push("--unified=3".to_string());
    }
    if staged {
        args.push("--cached".to_string());
    }
    if let Some(base) = base {
        args.push(base.to_string());
    }
    args.push("--".to_string());
    if let Some(pathspec) = pathspec {
        args.push(pathspec.to_string());
    }
    args
}

fn git_pathspec(cwd: &Path, ctx: &ToolContext<'_>) -> Result<Option<String>, ToolError> {
    let output = run_git(cwd, &["rev-parse", "--show-prefix"], ctx, 16 * 1024)?;
    if !output.success() {
        return Err(git_failure("resolve Git path filter", &output));
    }
    let prefix = String::from_utf8_lossy(&output.stdout)
        .trim()
        .replace('\\', "/");
    Ok((!prefix.is_empty()).then_some(prefix))
}

fn resolve_directory(args: &Value, ctx: &ToolContext<'_>) -> Result<PathBuf, ToolError> {
    let given = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let cwd = ctx.workspace.resolve(given);
    if cwd.is_dir() {
        Ok(cwd)
    } else {
        Err(ToolError::new(
            ToolErrorCode::NotDirectory,
            format!("{} is not a directory", cwd.display()),
        ))
    }
}

fn git_repo_root(cwd: &Path, ctx: &ToolContext<'_>) -> Result<Option<PathBuf>, ToolError> {
    let output = run_git(cwd, &["rev-parse", "--show-toplevel"], ctx, 16 * 1024)?;
    if !output.success() {
        return Ok(None);
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!root.is_empty()).then(|| PathBuf::from(root)))
}

fn git_text(cwd: &Path, args: &[&str], ctx: &ToolContext<'_>) -> Result<Option<String>, ToolError> {
    let output = run_git(cwd, args, ctx, 16 * 1024)?;
    if !output.success() {
        return Ok(None);
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!value.is_empty()).then_some(value))
}

fn run_git(
    cwd: &Path,
    args: &[&str],
    ctx: &ToolContext<'_>,
    max_bytes: usize,
) -> Result<ProcessOutput, ToolError> {
    let mut command = Command::new("git");
    command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(["-c", "safe.bareRepository=explicit"])
        .args(["-c", &format!("core.hooksPath={DISABLED_HOOKS_PATH}")])
        .args(["-c", "core.fsmonitor=false"])
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    run_process(command, ctx, max_bytes)
}

fn git_failure(action: &str, output: &ProcessOutput) -> ToolError {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let detail = if stderr.is_empty() {
        "Git returned a non-zero exit status"
    } else {
        &stderr
    };
    ToolError::execution(format!("failed to {action}: {detail}"))
}

struct StatusEntry {
    path: String,
    status: String,
}

struct ParsedStatus {
    entries: Vec<StatusEntry>,
    total: usize,
    staged: usize,
    unstaged: usize,
    untracked: usize,
    truncated: bool,
}

#[derive(Debug)]
struct DiffFileSummary {
    path: String,
    old_path: Option<String>,
    additions: Option<u64>,
    deletions: Option<u64>,
    binary: bool,
}

#[derive(Debug)]
struct DiffSummary {
    entries: Vec<DiffFileSummary>,
    total: usize,
    additions: u64,
    deletions: u64,
    binary_files: usize,
    truncated: bool,
}

fn parse_status(bytes: &[u8], process_truncated: bool) -> ParsedStatus {
    let mut entries = Vec::new();
    let mut total = 0usize;
    let mut staged = 0usize;
    let mut unstaged = 0usize;
    let mut untracked = 0usize;
    let mut fields = bytes.split(|byte| *byte == 0);
    while let Some(field) = fields.next() {
        if field.len() < 3 {
            continue;
        }
        let x = field[0] as char;
        let y = field[1] as char;
        let path = String::from_utf8_lossy(&field[3..]).into_owned();
        let renamed_or_copied = matches!(x, 'R' | 'C');
        let old_path = renamed_or_copied
            .then(|| fields.next())
            .flatten()
            .map(|old| String::from_utf8_lossy(old).into_owned());
        total += 1;
        if x == '?' && y == '?' {
            untracked += 1;
        } else {
            if x != ' ' {
                staged += 1;
            }
            if y != ' ' {
                unstaged += 1;
            }
        }
        if entries.len() < CHANGED_PATH_LIMIT {
            let path = old_path.map_or(path.clone(), |old| format!("{old} -> {path}"));
            entries.push(StatusEntry {
                path,
                status: format!("{x}{y}"),
            });
        }
    }
    ParsedStatus {
        truncated: process_truncated || total > entries.len(),
        entries,
        total,
        staged,
        unstaged,
        untracked,
    }
}

fn parse_numstat(bytes: &[u8], process_truncated: bool) -> DiffSummary {
    let mut entries = Vec::new();
    let mut total = 0usize;
    let mut additions = 0u64;
    let mut deletions = 0u64;
    let mut binary_files = 0usize;
    let mut fields = bytes.split(|byte| *byte == 0);

    while let Some(field) = fields.next() {
        if field.is_empty() {
            continue;
        }
        let header = String::from_utf8_lossy(field);
        let mut parts = header.splitn(3, '\t');
        let Some(added_text) = parts.next() else {
            continue;
        };
        let Some(deleted_text) = parts.next() else {
            continue;
        };
        let Some(path_text) = parts.next() else {
            continue;
        };
        let additions_for_file = added_text.parse::<u64>().ok();
        let deletions_for_file = deleted_text.parse::<u64>().ok();
        let binary = additions_for_file.is_none() || deletions_for_file.is_none();
        let (old_path, path) = if path_text.is_empty() {
            let Some(old_path) = fields.next() else {
                break;
            };
            let Some(path) = fields.next() else { break };
            (
                Some(String::from_utf8_lossy(old_path).replace('\\', "/")),
                String::from_utf8_lossy(path).replace('\\', "/"),
            )
        } else {
            (None, path_text.replace('\\', "/"))
        };

        total += 1;
        additions = additions.saturating_add(additions_for_file.unwrap_or(0));
        deletions = deletions.saturating_add(deletions_for_file.unwrap_or(0));
        binary_files += usize::from(binary);
        if entries.len() < DIFF_SUMMARY_ENTRY_LIMIT {
            entries.push(DiffFileSummary {
                path,
                old_path,
                additions: additions_for_file,
                deletions: deletions_for_file,
                binary,
            });
        }
    }

    DiffSummary {
        truncated: process_truncated || total > entries.len(),
        entries,
        total,
        additions,
        deletions,
        binary_files,
    }
}

fn render_diff_summary(summary: &DiffSummary) -> (String, bool) {
    let mut output = format!(
        "Changed files: {} (+{} -{}, {} binary)",
        summary.total, summary.additions, summary.deletions, summary.binary_files
    );
    let mut rendered_entries = 0usize;
    for entry in &summary.entries {
        let counts = if entry.binary {
            "binary".to_string()
        } else {
            format!(
                "+{} -{}",
                entry.additions.unwrap_or(0),
                entry.deletions.unwrap_or(0)
            )
        };
        let path = entry.old_path.as_ref().map_or_else(
            || entry.path.clone(),
            |old_path| format!("{old_path} -> {}", entry.path),
        );
        let path = truncate_summary_path(&path);
        let line = format!("\n- {counts} {path}");
        if output.chars().count() + line.chars().count() > DIFF_SUMMARY_MODEL_MAX_CHARS {
            break;
        }
        output.push_str(&line);
        rendered_entries += 1;
    }
    let truncated = summary.truncated || rendered_entries < summary.total;
    if truncated {
        output.push_str("\n[file summary truncated; use path to inspect a specific directory]");
    }
    (output, truncated)
}

fn truncate_summary_path(path: &str) -> String {
    const MAX_CHARS: usize = 240;
    if path.chars().count() <= MAX_CHARS {
        return path.to_string();
    }
    let prefix = path.chars().take(MAX_CHARS - 3).collect::<String>();
    format!("{prefix}...")
}

fn bound_diff(diff: &str, max_chars: usize) -> (String, bool) {
    if diff.chars().count() <= max_chars {
        return (diff.to_string(), false);
    }
    let prefix: String = diff.chars().take(max_chars).collect();
    (format!("{prefix}\n[diff truncated]"), true)
}

#[derive(Clone, Copy)]
enum Stream {
    Stdout,
    Stderr,
}

struct Chunk {
    stream: Stream,
    bytes: Vec<u8>,
}

struct ProcessOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_code: Option<i32>,
    truncated: bool,
}

impl ProcessOutput {
    fn success(&self) -> bool {
        self.exit_code == Some(0)
    }
}

fn run_process(
    mut command: Command,
    ctx: &ToolContext<'_>,
    max_bytes: usize,
) -> Result<ProcessOutput, ToolError> {
    let mut child = command
        .spawn()
        .map_err(|error| ToolError::execution(format!("failed to start Git: {error}")))?;
    let (sender, receiver) = mpsc::channel();
    let stdout_reader = spawn_reader(child.stdout.take(), Stream::Stdout, sender.clone());
    let stderr_reader = spawn_reader(child.stderr.take(), Stream::Stderr, sender);
    collect_process(
        &mut child,
        receiver,
        ctx,
        max_bytes,
        stdout_reader,
        stderr_reader,
    )
}

fn spawn_reader<R: Read + Send + 'static>(
    pipe: Option<R>,
    stream: Stream,
    sender: Sender<Chunk>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let Some(mut pipe) = pipe else { return };
        loop {
            let mut buffer = vec![0; 8 * 1024];
            let Ok(read) = pipe.read(&mut buffer) else {
                return;
            };
            if read == 0 {
                return;
            }
            buffer.truncate(read);
            if sender
                .send(Chunk {
                    stream: match stream {
                        Stream::Stdout => Stream::Stdout,
                        Stream::Stderr => Stream::Stderr,
                    },
                    bytes: buffer,
                })
                .is_err()
            {
                return;
            }
        }
    })
}

fn collect_process(
    child: &mut Child,
    receiver: Receiver<Chunk>,
    ctx: &ToolContext<'_>,
    max_bytes: usize,
    stdout_reader: std::thread::JoinHandle<()>,
    stderr_reader: std::thread::JoinHandle<()>,
) -> Result<ProcessOutput, ToolError> {
    let started = Instant::now();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut truncated = false;
    let mut termination = None;
    loop {
        drain_chunks(
            &receiver,
            &mut stdout,
            &mut stderr,
            max_bytes,
            &mut truncated,
        );
        if truncated {
            let _ = child.kill();
            termination = Some("output exceeded its limit");
        }
        if ctx.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = child.kill();
            termination = Some("cancelled");
        }
        if started.elapsed() > GIT_TIMEOUT {
            let _ = child.kill();
            termination = Some("timed out");
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                drain_chunks(
                    &receiver,
                    &mut stdout,
                    &mut stderr,
                    max_bytes,
                    &mut truncated,
                );
                return match termination {
                    Some("cancelled") => Err(ToolError::new(
                        ToolErrorCode::Aborted,
                        "Git query was cancelled",
                    )),
                    Some("timed out") => Err(ToolError::new(
                        ToolErrorCode::Timeout,
                        "Git query timed out",
                    )),
                    _ => Ok(ProcessOutput {
                        stdout,
                        stderr,
                        exit_code: status.code(),
                        truncated,
                    }),
                };
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(error) => {
                return Err(ToolError::execution(format!(
                    "failed while waiting for Git: {error}"
                )))
            }
        }
    }
}

fn drain_chunks(
    receiver: &Receiver<Chunk>,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
    max_bytes: usize,
    truncated: &mut bool,
) {
    while let Ok(chunk) = receiver.try_recv() {
        let combined = stdout.len() + stderr.len();
        if combined >= max_bytes {
            *truncated = true;
            continue;
        }
        let available = max_bytes - combined;
        if chunk.bytes.len() > available {
            match chunk.stream {
                Stream::Stdout => stdout.extend_from_slice(&chunk.bytes[..available]),
                Stream::Stderr => stderr.extend_from_slice(&chunk.bytes[..available]),
            }
            *truncated = true;
        } else {
            match chunk.stream {
                Stream::Stdout => stdout.extend_from_slice(&chunk.bytes),
                Stream::Stderr => stderr.extend_from_slice(&chunk.bytes),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn parses_porcelain_status_with_rename_records() {
        let parsed = parse_status(b" M src/main.rs\0?? new.txt\0R  old.rs\0new.rs\0", false);
        assert_eq!(parsed.total, 3);
        assert_eq!(parsed.staged, 1);
        assert_eq!(parsed.unstaged, 1);
        assert_eq!(parsed.untracked, 1);
        assert!(parsed
            .entries
            .iter()
            .any(|entry| entry.path == "new.rs -> old.rs"));
    }

    #[test]
    fn bounds_large_diff() {
        let source = "x".repeat(DIFF_MODEL_MAX_CHARS + 1);
        let (bounded, truncated) = bound_diff(&source, DIFF_MODEL_MAX_CHARS);
        assert!(truncated);
        assert!(bounded.ends_with("[diff truncated]"));
    }

    #[test]
    fn parses_text_binary_and_rename_numstat_records() {
        let parsed = parse_numstat(
            b"10\t2\tsrc/main.rs\0-\t-\timage.png\01\t0\t\0old.rs\0new.rs\0",
            false,
        );
        assert_eq!(parsed.total, 3);
        assert_eq!(parsed.additions, 11);
        assert_eq!(parsed.deletions, 2);
        assert_eq!(parsed.binary_files, 1);
        assert_eq!(parsed.entries[0].path, "src/main.rs");
        assert!(parsed.entries[1].binary);
        assert_eq!(parsed.entries[2].old_path.as_deref(), Some("old.rs"));
        assert_eq!(parsed.entries[2].path, "new.rs");
    }

    fn temp_repo() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "onemore-git-tool-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.name", "Onemore Tests"],
            vec!["config", "user.email", "onemore-tests@example.invalid"],
        ] {
            let status = Command::new("git")
                .args(args)
                .current_dir(&root)
                .status()
                .unwrap();
            assert!(status.success());
        }
        std::fs::write(root.join("tracked.txt"), "before\n").unwrap();
        let status = Command::new("git")
            .args(["add", "tracked.txt"])
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args(["commit", "-qm", "initial"])
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::write(root.join("tracked.txt"), "after\n").unwrap();
        std::fs::write(root.join("untracked.txt"), "new\n").unwrap();
        root
    }

    fn run<T: Tool>(tool: &T, root: &Path, args: Value) -> Result<ToolOutput, ToolError> {
        let workspace = Workspace::new(root.to_path_buf());
        let cancel = AtomicBool::new(false);
        tool.execute(
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
    fn repo_state_and_diff_use_read_only_git_queries() {
        let root = temp_repo();
        let state = run(&RepoState, &root, json!({})).unwrap();
        assert!(!state.model_text.contains('\\'));
        let details = state.details.unwrap();
        assert_eq!(details["is_git_repository"], true);
        assert_eq!(details["changed_path_count"], 2);
        assert_eq!(details["unstaged_count"], 1);
        assert_eq!(details["untracked_count"], 1);

        let diff = run(&GitDiff, &root, json!({})).unwrap();
        assert!(diff.model_text.starts_with("Changed files: 1 (+1 -1"));
        assert!(diff.model_text.contains("-before"));
        assert!(diff.model_text.contains("+after"));
        let details = diff.details.unwrap();
        assert_eq!(details["changed_file_count"], 1);
        assert_eq!(details["changed_files"][0]["path"], "tracked.txt");
        assert_eq!(details["truncated"], false);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn diff_path_filters_summary_and_patch() {
        let root = temp_repo();
        std::fs::create_dir_all(root.join("src").join("tools")).unwrap();
        std::fs::write(root.join("src").join("tools").join("scoped.rs"), "before\n").unwrap();
        std::fs::write(root.join("other.rs"), "before\n").unwrap();
        for args in [vec!["add", "."], vec!["commit", "-qm", "scope baseline"]] {
            let status = Command::new("git")
                .args(args)
                .current_dir(&root)
                .status()
                .unwrap();
            assert!(status.success());
        }
        std::fs::write(root.join("src").join("tools").join("scoped.rs"), "after\n").unwrap();
        std::fs::write(root.join("other.rs"), "after\n").unwrap();

        let diff = run(&GitDiff, &root, json!({ "path": "src/tools" })).unwrap();
        assert!(diff.model_text.contains("src/tools/scoped.rs"));
        assert!(diff.model_text.contains("+after"));
        assert!(!diff.model_text.contains("other.rs"));
        let details = diff.details.unwrap();
        assert_eq!(details["pathspec"], "src/tools/");
        assert_eq!(details["changed_file_count"], 1);
        assert_eq!(details["changed_files"][0]["path"], "src/tools/scoped.rs");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn non_repository_is_a_structured_empty_state() {
        let root = std::env::temp_dir().join(format!(
            "onemore-non-git-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let state = run(&RepoState, &root, json!({})).unwrap();
        assert_eq!(state.details.unwrap()["is_git_repository"], false);
        let _ = std::fs::remove_dir_all(root);
    }
}
