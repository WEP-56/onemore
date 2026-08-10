use std::path::{Path, PathBuf};

use crate::tools::{CommandPermissionSpec, CommandSyntax, PreparedToolCall};
use crate::workspace::Workspace;

use super::{canonicalize_nearest, path_starts_with, ApprovalDetails};

pub(super) struct CommandAssessment {
    pub requires_approval: bool,
    pub reasons: Vec<String>,
    pub details: ApprovalDetails,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(String),
    Separator,
}

#[derive(Default)]
struct RiskState {
    reasons: Vec<String>,
    targets: Vec<String>,
}

pub(super) fn assess(
    call: &PreparedToolCall,
    spec: &CommandPermissionSpec,
    workspace: &Workspace,
) -> Result<CommandAssessment, String> {
    let command = call
        .arguments
        .get(&spec.argument)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("命令参数 {:?} 不是字符串", spec.argument))?;
    let cwd = match spec
        .cwd_argument
        .as_ref()
        .and_then(|name| call.arguments.get(name))
    {
        Some(value) => {
            let given = value
                .as_str()
                .ok_or_else(|| "cwd 参数不是字符串".to_string())?;
            workspace.resolve(given)
        }
        None => workspace.root().to_path_buf(),
    };
    let canonical_cwd = canonicalize_nearest(&cwd)
        .map_err(|error| format!("无法解析 cwd {}: {error}", cwd.display()))?;
    let canonical_root = canonicalize_nearest(workspace.root()).map_err(|error| {
        format!(
            "无法解析 workspace 根目录 {}: {error}",
            workspace.root().display()
        )
    })?;

    let tokens = lex(command, spec.syntax)?;
    let mut state = RiskState::default();
    inspect_tokens(
        &tokens,
        spec.syntax,
        &canonical_cwd,
        &canonical_root,
        &mut state,
        0,
    )?;
    dedup(&mut state.reasons);
    dedup(&mut state.targets);

    Ok(CommandAssessment {
        requires_approval: !state.reasons.is_empty(),
        reasons: state.reasons,
        details: ApprovalDetails {
            command: Some(command.trim().to_string()),
            cwd: Some(canonical_cwd.display().to_string()),
            targets: state.targets,
        },
    })
}

fn inspect_tokens(
    tokens: &[Token],
    syntax: CommandSyntax,
    cwd: &Path,
    workspace_root: &Path,
    state: &mut RiskState,
    depth: usize,
) -> Result<(), String> {
    if depth > 4 {
        return Err("嵌套 shell 层级超过安全分析上限".into());
    }
    for segment in split_segments(tokens) {
        inspect_segment(&segment, syntax, cwd, workspace_root, state, depth)?;
    }
    Ok(())
}

fn inspect_segment(
    segment: &[String],
    syntax: CommandSyntax,
    cwd: &Path,
    workspace_root: &Path,
    state: &mut RiskState,
    depth: usize,
) -> Result<(), String> {
    if segment.is_empty() {
        return Ok(());
    }
    let mut index = 0;
    if syntax == CommandSyntax::Posix {
        while segment
            .get(index)
            .is_some_and(|word| is_environment_assignment(word))
        {
            index += 1;
        }
        while let Some(wrapper) = segment.get(index).map(|word| command_name(word)) {
            if !matches!(
                wrapper.as_str(),
                "sudo" | "doas" | "command" | "builtin" | "nohup" | "env"
            ) {
                break;
            }
            index += 1;
            while segment
                .get(index)
                .is_some_and(|word| word.starts_with('-') || is_environment_assignment(word))
            {
                index += 1;
            }
        }
    }
    let Some(raw_name) = segment.get(index) else {
        return Ok(());
    };
    if is_dynamic(raw_name, syntax) {
        return Err(format!("命令入口 {:?} 是动态表达式", raw_name));
    }
    let name = command_name(raw_name);
    let args = &segment[index + 1..];

    if matches!(name.as_str(), "eval" | "invoke-expression" | "iex") {
        return Err(format!("{} 会动态执行无法静态验证的命令", raw_name));
    }
    if let Some((nested_syntax, nested_command)) = nested_shell(&name, args)? {
        let nested = lex(nested_command, nested_syntax)?;
        inspect_tokens(
            &nested,
            nested_syntax,
            cwd,
            workspace_root,
            state,
            depth + 1,
        )?;
    }

    if name == "xargs" {
        if let Some(nested) = args.iter().find(|arg| !arg.starts_with('-')) {
            let mut nested_words = vec![nested.clone()];
            let start = args.iter().position(|arg| arg == nested).unwrap_or(0) + 1;
            nested_words.extend_from_slice(&args[start..]);
            inspect_segment(&nested_words, syntax, cwd, workspace_root, state, depth + 1)?;
        } else {
            return Err("xargs 未提供可静态识别的目标命令".into());
        }
    }
    if name == "find" {
        for marker in ["-exec", "-execdir", "-delete"] {
            if let Some(position) = args.iter().position(|arg| arg.eq_ignore_ascii_case(marker)) {
                if marker == "-delete" {
                    record_risk(
                        state,
                        "find -delete 会递归删除匹配项",
                        args,
                        cwd,
                        workspace_root,
                        true,
                    );
                } else if let Some(command) = args.get(position + 1) {
                    inspect_segment(
                        &[command.clone()],
                        syntax,
                        cwd,
                        workspace_root,
                        state,
                        depth + 1,
                    )?;
                } else {
                    return Err(format!("{marker} 缺少目标命令"));
                }
            }
        }
    }

    let recursive = has_recursive_flag(args, syntax);
    match name.as_str() {
        "rm" | "rmdir" | "unlink" | "del" | "erase" | "rd" | "remove-item" | "ri" => record_risk(
            state,
            format!("{} 会删除文件或目录", raw_name),
            args,
            cwd,
            workspace_root,
            recursive,
        ),
        "git" => inspect_git(args, cwd, workspace_root, state),
        "chmod" | "chown" | "chgrp" | "setfacl" | "icacls" | "takeown" | "set-acl" => record_risk(
            state,
            format!("{} 会修改权限或所有者", raw_name),
            args,
            cwd,
            workspace_root,
            recursive,
        ),
        "mkfs" | "diskpart" | "clear-disk" | "initialize-disk" | "remove-partition" | "format"
        | "format.com" | "dd" => record_risk(
            state,
            format!("{} 可能格式化、覆盖或清理磁盘", raw_name),
            args,
            cwd,
            workspace_root,
            true,
        ),
        "reg"
            if args.first().is_some_and(|arg| {
                matches!(
                    arg.to_ascii_lowercase().as_str(),
                    "add" | "delete" | "import"
                )
            }) =>
        {
            record_risk(
                state,
                "reg 会修改系统注册表",
                args,
                cwd,
                workspace_root,
                recursive,
            )
        }
        "sc" | "sc.exe"
            if args.first().is_some_and(|arg| {
                matches!(
                    arg.to_ascii_lowercase().as_str(),
                    "config" | "delete" | "create"
                )
            }) =>
        {
            record_risk(
                state,
                "sc 会修改系统服务配置",
                args,
                cwd,
                workspace_root,
                recursive,
            )
        }
        "set-executionpolicy" | "bcdedit" | "netsh" => record_risk(
            state,
            format!("{} 会修改系统级配置", raw_name),
            args,
            cwd,
            workspace_root,
            recursive,
        ),
        _ => {}
    }
    Ok(())
}

fn inspect_git(args: &[String], cwd: &Path, workspace_root: &Path, state: &mut RiskState) {
    let Some(subcommand_index) = args.iter().position(|arg| !arg.starts_with('-')) else {
        return;
    };
    let subcommand = args[subcommand_index].to_ascii_lowercase();
    let rest = &args[subcommand_index + 1..];
    if subcommand == "clean" {
        record_risk(
            state,
            "git clean 会不可逆删除未跟踪文件",
            rest,
            cwd,
            workspace_root,
            true,
        );
        if state.targets.is_empty() {
            state
                .targets
                .push(format!("{} [repository scope]", cwd.display()));
        }
    } else if subcommand == "reset" && rest.iter().any(|arg| arg.eq_ignore_ascii_case("--hard")) {
        state
            .reasons
            .push("git reset --hard 会不可逆丢弃工作区修改".into());
        state
            .targets
            .push(format!("{} [repository scope]", cwd.display()));
    }
}

fn record_risk(
    state: &mut RiskState,
    reason: impl Into<String>,
    args: &[String],
    cwd: &Path,
    workspace_root: &Path,
    recursive: bool,
) {
    state.reasons.push(reason.into());
    let mut found = false;
    for target in target_arguments(args) {
        found = true;
        state
            .targets
            .push(describe_target(target, cwd, workspace_root, recursive));
    }
    if !found {
        state
            .targets
            .push(format!("{} [target scope unresolved]", cwd.display()));
    }
}

fn target_arguments(args: &[String]) -> impl Iterator<Item = &str> {
    args.iter().filter_map(|arg| {
        let lower = arg.to_ascii_lowercase();
        if arg.starts_with('-')
            || (arg.starts_with('/') && is_cmd_switch(&lower))
            || matches!(
                lower.as_str(),
                "clean" | "reset" | "add" | "delete" | "config" | "create" | "import"
            )
        {
            None
        } else {
            Some(arg.as_str())
        }
    })
}

fn describe_target(raw: &str, cwd: &Path, workspace_root: &Path, recursive: bool) -> String {
    let wildcard = raw.contains(['*', '?', '[', ']']);
    let dynamic = raw.contains('$') || raw.contains('%') || raw.contains('`');
    let base = wildcard_base(raw);
    let candidate = if Path::new(base).is_absolute() {
        PathBuf::from(base)
    } else {
        cwd.join(base)
    };
    let canonical = canonicalize_nearest(&candidate).unwrap_or(candidate);
    let outside = !path_starts_with(&canonical, workspace_root);
    let mut flags = Vec::new();
    if recursive {
        flags.push("recursive");
    }
    if wildcard {
        flags.push("wildcard");
    }
    if outside {
        flags.push("outside workspace");
    }
    if dynamic {
        flags.push("dynamic path");
    }
    let suffix = if flags.is_empty() {
        String::new()
    } else {
        format!(" [{}]", flags.join(", "))
    };
    format!("{} -> {}{}", raw, canonical.display(), suffix)
}

fn wildcard_base(raw: &str) -> &str {
    let boundary = raw
        .char_indices()
        .find(|(_, ch)| matches!(ch, '*' | '?' | '[' | ']'))
        .map(|(index, _)| index)
        .unwrap_or(raw.len());
    let prefix = &raw[..boundary];
    prefix
        .rfind(['/', '\\'])
        .map(|index| &prefix[..=index])
        .filter(|value| !value.is_empty())
        .unwrap_or(".")
}

fn split_segments(tokens: &[Token]) -> Vec<Vec<String>> {
    let mut segments = vec![Vec::new()];
    for token in tokens {
        match token {
            Token::Word(word) => segments.last_mut().unwrap().push(word.clone()),
            Token::Separator => {
                if segments.last().is_some_and(|segment| !segment.is_empty()) {
                    segments.push(Vec::new());
                }
            }
        }
    }
    segments.retain(|segment| !segment.is_empty());
    segments
}

fn lex(input: &str, syntax: CommandSyntax) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let chars: Vec<char> = input.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if escaped {
            word.push(ch);
            escaped = false;
            index += 1;
            continue;
        }
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            } else if ch == '\\' && active == '"' && syntax != CommandSyntax::PowerShell {
                escaped = true;
            } else if ch == '`' && syntax == CommandSyntax::PowerShell {
                escaped = true;
            } else {
                word.push(ch);
            }
            index += 1;
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '\\' if syntax == CommandSyntax::Posix => escaped = true,
            '`' => return Err("命令包含动态反引号表达式".into()),
            '$' if chars.get(index + 1) == Some(&'(') => {
                return Err("命令包含动态子命令表达式 $(...)".into())
            }
            ' ' | '\t' | '\r' => flush_word(&mut tokens, &mut word),
            '\n' | ';' | '|' | '&' | '(' | ')' => {
                flush_word(&mut tokens, &mut word);
                if !matches!(tokens.last(), Some(Token::Separator)) {
                    tokens.push(Token::Separator);
                }
                if matches!(ch, '|' | '&') && chars.get(index + 1) == Some(&ch) {
                    index += 1;
                }
            }
            _ => word.push(ch),
        }
        index += 1;
    }
    if quote.is_some() {
        return Err("命令包含未闭合引号".into());
    }
    if escaped {
        return Err("命令以未完成的转义符结尾".into());
    }
    flush_word(&mut tokens, &mut word);
    Ok(tokens)
}

fn flush_word(tokens: &mut Vec<Token>, word: &mut String) {
    if !word.is_empty() {
        tokens.push(Token::Word(std::mem::take(word)));
    }
}

fn command_name(raw: &str) -> String {
    raw.rsplit(['/', '\\'])
        .next()
        .unwrap_or(raw)
        .trim_matches(['\'', '"'])
        .trim_end_matches(".exe")
        .to_ascii_lowercase()
}

fn nested_shell<'a>(
    name: &str,
    args: &'a [String],
) -> Result<Option<(CommandSyntax, &'a str)>, String> {
    let syntax = match name {
        "sh" | "bash" | "zsh" | "dash" => Some(CommandSyntax::Posix),
        "powershell" | "pwsh" => Some(CommandSyntax::PowerShell),
        "cmd" => Some(CommandSyntax::Cmd),
        _ => None,
    };
    let Some(syntax) = syntax else {
        return Ok(None);
    };
    let marker = match syntax {
        CommandSyntax::Posix => args.iter().position(|arg| arg == "-c"),
        CommandSyntax::PowerShell => args.iter().position(|arg| {
            matches!(
                arg.to_ascii_lowercase().as_str(),
                "-command" | "-c" | "/command"
            )
        }),
        CommandSyntax::Cmd => args.iter().position(|arg| arg.eq_ignore_ascii_case("/c")),
    };
    if args.iter().any(|arg| {
        matches!(
            arg.to_ascii_lowercase().as_str(),
            "-encodedcommand" | "-enc"
        )
    }) {
        return Err(format!("{name} 使用编码命令，无法静态验证"));
    }
    match marker {
        Some(index) => args
            .get(index + 1)
            .map(|command| Some((syntax, command.as_str())))
            .ok_or_else(|| format!("{name} 的命令参数缺失")),
        None => Ok(None),
    }
}

fn has_recursive_flag(args: &[String], syntax: CommandSyntax) -> bool {
    args.iter().any(|arg| {
        let lower = arg.to_ascii_lowercase();
        lower == "--recursive"
            || lower == "-recurse"
            || lower == "/s"
            || (syntax != CommandSyntax::PowerShell
                && lower.starts_with('-')
                && lower[1..].chars().any(|ch| ch == 'r'))
    })
}

fn is_cmd_switch(value: &str) -> bool {
    matches!(value, "/s" | "/q" | "/f" | "/a" | "/p")
}

fn is_environment_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn is_dynamic(word: &str, syntax: CommandSyntax) -> bool {
    word.starts_with('$')
        || word.contains("$(")
        || word.contains('`')
        || (syntax == CommandSyntax::Cmd && word.starts_with('%'))
}

fn dedup(values: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    values.retain(|value| seen.insert(value.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{CommandPermissionSpec, CommandSyntax, Tool, ToolCapabilities, ToolContext};
    use crate::tools::{ToolError, ToolOutput, ToolPermissionSpec, ToolRegistry, ToolSpec};
    use serde_json::{json, Value};

    struct CommandTool(CommandSyntax);

    impl Tool for CommandTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "command_test".into(),
                description: String::new(),
                schema: json!({"type":"object","properties":{"command":{"type":"string"},"cwd":{"type":"string"}},"required":["command"]}),
                capabilities: ToolCapabilities::COMMAND,
                permission: ToolPermissionSpec::command("command", Some("cwd"), self.0),
            }
        }

        fn execute(&self, _: &Value, _: &mut ToolContext<'_>) -> Result<ToolOutput, ToolError> {
            unreachable!()
        }
    }

    fn assessment(syntax: CommandSyntax, command: &str) -> CommandAssessment {
        let root = std::env::temp_dir().join(format!("onemore-risk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let registry = ToolRegistry::new(vec![Box::new(CommandTool(syntax))]);
        let prepared = registry
            .prepare("command_test", &json!({"command": command}))
            .unwrap();
        let result = assess(
            &prepared,
            &CommandPermissionSpec {
                argument: "command".into(),
                cwd_argument: Some("cwd".into()),
                syntax,
            },
            &Workspace::new(root.clone()),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(root);
        result
    }

    #[test]
    fn detects_deletes_in_chains_pipelines_and_nested_shells() {
        for (syntax, command) in [
            (CommandSyntax::Posix, "echo ok && rm -rf build"),
            (
                CommandSyntax::PowerShell,
                "Get-ChildItem | Remove-Item -Recurse",
            ),
            (CommandSyntax::Cmd, "echo ok & del /s build\\*"),
            (CommandSyntax::Posix, "bash -c 'rm -f generated.txt'"),
        ] {
            let result = assessment(syntax, command);
            assert!(result.requires_approval, "未识别: {command}");
            assert!(!result.details.targets.is_empty());
        }
    }

    #[test]
    fn detects_git_cleanup_permissions_and_disk_operations() {
        for command in [
            "git clean -fdx",
            "git reset --hard HEAD",
            "chmod -R 777 .",
            "diskpart /s wipe.txt",
        ] {
            assert!(assessment(CommandSyntax::Posix, command).requires_approval);
        }
    }

    #[test]
    fn ordinary_commands_are_not_forced() {
        for command in ["cargo test", "git status --short", "echo rm"] {
            assert!(!assessment(CommandSyntax::Posix, command).requires_approval);
        }
    }

    #[test]
    fn malformed_or_dynamic_commands_fail_closed() {
        for command in [
            "echo 'unterminated",
            "$(printf rm) -rf build",
            "eval $ACTION",
        ] {
            let root = std::env::temp_dir().join(format!("onemore-risk-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let registry = ToolRegistry::new(vec![Box::new(CommandTool(CommandSyntax::Posix))]);
            let prepared = registry
                .prepare("command_test", &json!({"command": command}))
                .unwrap();
            assert!(assess(
                &prepared,
                &CommandPermissionSpec {
                    argument: "command".into(),
                    cwd_argument: Some("cwd".into()),
                    syntax: CommandSyntax::Posix,
                },
                &Workspace::new(root.clone())
            )
            .is_err());
            let _ = std::fs::remove_dir_all(root);
        }
    }
}
