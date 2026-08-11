//! 工具权限策略：只消费 schema 校验后的 [`PreparedToolCall`]，不认识具体工具名。
//!
//! 策略顺序固定为：路径 hard deny -> 配置规则 -> 已批准的会话级精确调用。Hook 只能在
//! 两次策略检查之间收窄参数，Runtime 会对替换后的参数重新 preflight，因此 Hook 的
//! Allow 或参数改写都不能覆盖 hard deny。

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::tools::PreparedToolCall;
use crate::workspace::Workspace;

mod command_risk;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionRule {
    Allow,
    Ask,
    Deny,
}

impl PermissionRule {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "allow" => Ok(PermissionRule::Allow),
            "ask" => Ok(PermissionRule::Ask),
            "deny" => Ok(PermissionRule::Deny),
            other => Err(format!("未知权限规则 {:?}，可选 allow | ask | deny", other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionRules {
    pub workspace_read: PermissionRule,
    pub workspace_write: PermissionRule,
    pub outside_workspace: PermissionRule,
    pub opaque_side_effect: PermissionRule,
}

impl Default for PermissionRules {
    fn default() -> Self {
        PermissionRules {
            workspace_read: PermissionRule::Allow,
            workspace_write: PermissionRule::Allow,
            outside_workspace: PermissionRule::Ask,
            opaque_side_effect: PermissionRule::Ask,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalScope {
    Once,
    Session,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Allow(ApprovalScope),
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalResponse {
    pub request_id: String,
    pub decision: ApprovalDecision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub request_id: String,
    pub tool: String,
    pub summary: String,
    pub reason: String,
    pub scopes: Vec<ApprovalScope>,
    pub details: ApprovalDetails,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApprovalDetails {
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    Deny {
        reason: String,
    },
    Ask {
        reason: String,
        scopes: Vec<ApprovalScope>,
        details: ApprovalDetails,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathAssessment {
    pub requested: PathBuf,
    pub canonical: PathBuf,
    pub within_workspace: bool,
}

pub struct PermissionManager {
    rules: PermissionRules,
    session_grants: HashSet<String>,
}

impl PermissionManager {
    pub fn new(rules: PermissionRules) -> Self {
        PermissionManager {
            rules,
            session_grants: HashSet::new(),
        }
    }

    pub fn evaluate(&self, call: &PreparedToolCall, workspace: &Workspace) -> PermissionDecision {
        let mut reasons = Vec::new();
        let mut effective = PermissionRule::Allow;
        let mut has_declared_path = false;
        let mut forced_approval = false;
        let mut details = ApprovalDetails::default();

        for argument in &call.spec.permission.path_arguments {
            has_declared_path = true;
            let Some(value) = call.arguments.get(argument) else {
                continue;
            };
            let Some(given) = value.as_str() else {
                return PermissionDecision::Deny {
                    reason: format!("路径参数 {:?} 不是字符串", argument),
                };
            };
            let assessment = match assess_path(workspace, given) {
                Ok(assessment) => assessment,
                Err(reason) => return PermissionDecision::Deny { reason },
            };
            if !assessment.within_workspace {
                merge_rule(
                    &mut effective,
                    self.rules.outside_workspace,
                    &mut reasons,
                    format!("路径 {} 位于 workspace 外", assessment.canonical.display()),
                );
            }
        }

        if call.spec.permission.always_ask {
            forced_approval = true;
            reasons.push("工具声明了必须逐次审批且无法静态约束的副作用".into());
        }

        if let Some(command_spec) = &call.spec.permission.command {
            merge_rule(
                &mut effective,
                self.rules.opaque_side_effect,
                &mut reasons,
                "shell 命令执行".into(),
            );
            match command_risk::assess(call, command_spec, workspace) {
                Ok(assessment) => {
                    details = assessment.details;
                    if assessment.requires_approval {
                        forced_approval = true;
                        reasons.extend(assessment.reasons);
                    }
                }
                Err(reason) => {
                    forced_approval = true;
                    reasons.push(format!("命令无法可靠解析: {reason}"));
                }
            }
        } else if effective != PermissionRule::Deny {
            let workspace_rule = if call.spec.capabilities.read_only {
                self.rules.workspace_read
            } else {
                self.rules.workspace_write
            };
            let description = if call.spec.capabilities.read_only {
                "workspace 内读取"
            } else if has_declared_path {
                "workspace 内写入"
            } else {
                "未声明目标的副作用"
            };
            let rule = if call.spec.permission.always_ask {
                PermissionRule::Allow
            } else if !call.spec.capabilities.read_only && !has_declared_path {
                PermissionRule::Ask
            } else {
                workspace_rule
            };
            merge_rule(&mut effective, rule, &mut reasons, description.into());
        }

        match effective {
            PermissionRule::Deny => PermissionDecision::Deny {
                reason: reasons.join("；"),
            },
            _ if forced_approval => PermissionDecision::Ask {
                reason: reasons.join("；"),
                scopes: vec![ApprovalScope::Once],
                details,
            },
            PermissionRule::Ask if self.session_grants.contains(&grant_key(call)) => {
                PermissionDecision::Allow
            }
            PermissionRule::Ask => PermissionDecision::Ask {
                reason: reasons.join("；"),
                scopes: vec![ApprovalScope::Once, ApprovalScope::Session],
                details,
            },
            PermissionRule::Allow => PermissionDecision::Allow,
        }
    }

    pub fn remember_session_grant(&mut self, call: &PreparedToolCall) {
        self.session_grants.insert(grant_key(call));
    }

    pub fn clear_session_grants(&mut self) {
        self.session_grants.clear();
    }
}

pub fn assess_path(workspace: &Workspace, given: &str) -> Result<PathAssessment, String> {
    if let Some(reason) = hard_deny_path(given) {
        return Err(reason);
    }
    let requested = workspace.resolve(given);
    let canonical = canonicalize_nearest(&requested)
        .map_err(|error| format!("无法安全解析路径 {}: {}", requested.display(), error))?;
    let canonical_root = canonicalize_nearest(workspace.root()).map_err(|error| {
        format!(
            "无法安全解析 workspace 根目录 {}: {}",
            workspace.root().display(),
            error
        )
    })?;
    Ok(PathAssessment {
        requested,
        within_workspace: path_starts_with(&canonical, &canonical_root),
        canonical,
    })
}

fn merge_rule(
    effective: &mut PermissionRule,
    next: PermissionRule,
    reasons: &mut Vec<String>,
    reason: String,
) {
    if rank(next) > rank(*effective) {
        *effective = next;
    }
    if next != PermissionRule::Allow {
        reasons.push(reason);
    }
}

fn rank(rule: PermissionRule) -> u8 {
    match rule {
        PermissionRule::Allow => 0,
        PermissionRule::Ask => 1,
        PermissionRule::Deny => 2,
    }
}

fn grant_key(call: &PreparedToolCall) -> String {
    // 按名授权(MCP 远端工具):同名工具在 session 内一次审批即可,参数不参与
    // 匹配。默认仍是"工具名 + 完整参数"的精确授权;always_ask 工具不会走到
    // session grant,因此该声明放不宽高危边界。
    if call.spec.permission.session_grant_by_name {
        format!("{}\n<session-grant-by-name>", call.spec.name)
    } else {
        format!("{}\n{}", call.spec.name, call.arguments)
    }
}

fn canonicalize_nearest(path: &Path) -> std::io::Result<PathBuf> {
    let mut cursor = path.to_path_buf();
    let mut missing: Vec<OsString> = Vec::new();
    loop {
        match std::fs::canonicalize(&cursor) {
            Ok(mut canonical) => {
                for component in missing.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = cursor.file_name().map(OsString::from) else {
                    return Err(error);
                };
                missing.push(name);
                if !cursor.pop() {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
    }
}

fn path_starts_with(path: &Path, root: &Path) -> bool {
    let mut path_components = path.components();
    for root_component in root.components() {
        let Some(path_component) = path_components.next() else {
            return false;
        };
        if !components_equal(path_component, root_component) {
            return false;
        }
    }
    true
}

fn components_equal(left: Component<'_>, right: Component<'_>) -> bool {
    if cfg!(windows) {
        left.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
    } else {
        left == right
    }
}

fn hard_deny_path(given: &str) -> Option<String> {
    if given.contains('\0') {
        return Some("路径包含 NUL 字节".into());
    }
    if !cfg!(windows) {
        return None;
    }
    let normalized = given.replace('/', "\\").to_ascii_lowercase();
    if normalized.starts_with(r"\\.\")
        || normalized.starts_with(r"\\?\globalroot\")
        || normalized.starts_with(r"\\?\pipe\")
    {
        return Some(format!("禁止访问 Windows 设备路径 {:?}", given));
    }
    for component in Path::new(given).components() {
        let name = component
            .as_os_str()
            .to_string_lossy()
            .trim_end_matches(['.', ' '])
            .split('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let reserved = matches!(name.as_str(), "con" | "prn" | "aux" | "nul")
            || name
                .strip_prefix("com")
                .or_else(|| name.strip_prefix("lpt"))
                .is_some_and(|number| {
                    matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
                });
        if reserved {
            return Some(format!("禁止访问 Windows 保留设备名 {:?}", component));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{
        Tool, ToolCapabilities, ToolContext, ToolError, ToolOutput, ToolPermissionSpec,
        ToolRegistry, ToolSpec,
    };
    use serde_json::{json, Value};

    struct PolicyTool {
        capabilities: ToolCapabilities,
        permission: ToolPermissionSpec,
    }

    impl Tool for PolicyTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "policy_test".into(),
                description: "policy test".into(),
                schema: json!({
                    "type": "object",
                    "properties": { "path": { "type": "string" } }
                }),
                capabilities: self.capabilities,
                permission: self.permission.clone(),
            }
        }

        fn execute(
            &self,
            _args: &Value,
            _ctx: &mut ToolContext<'_>,
        ) -> Result<ToolOutput, ToolError> {
            unreachable!()
        }
    }

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "onemore-permission-{}-{}-{}",
            name,
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn prepared(
        capabilities: ToolCapabilities,
        permission: ToolPermissionSpec,
        args: Value,
    ) -> PreparedToolCall {
        ToolRegistry::new(vec![Box::new(PolicyTool {
            capabilities,
            permission,
        })])
        .prepare("policy_test", &args)
        .unwrap()
    }

    #[test]
    fn default_rules_allow_workspace_files_and_ask_for_opaque_or_external() {
        let root = temp_root("matrix");
        let outside = temp_root("outside").join("file.txt");
        let workspace = Workspace::new(root.clone());
        let manager = PermissionManager::new(PermissionRules::default());

        let read = prepared(
            ToolCapabilities::READ_ONLY,
            ToolPermissionSpec::paths(&["path"]),
            json!({ "path": "inside.txt" }),
        );
        assert_eq!(
            manager.evaluate(&read, &workspace),
            PermissionDecision::Allow
        );

        let write = prepared(
            ToolCapabilities::MUTATION,
            ToolPermissionSpec::paths(&["path"]),
            json!({ "path": "inside.txt" }),
        );
        assert_eq!(
            manager.evaluate(&write, &workspace),
            PermissionDecision::Allow
        );

        let external = prepared(
            ToolCapabilities::READ_ONLY,
            ToolPermissionSpec::paths(&["path"]),
            json!({ "path": outside }),
        );
        assert!(matches!(
            manager.evaluate(&external, &workspace),
            PermissionDecision::Ask { .. }
        ));

        let command = prepared(
            ToolCapabilities::COMMAND,
            ToolPermissionSpec::opaque_side_effect(&[]),
            json!({}),
        );
        assert!(matches!(
            manager.evaluate(&command, &workspace),
            PermissionDecision::Ask { .. }
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn exact_session_grant_does_not_authorize_different_arguments() {
        let root = temp_root("grant");
        let workspace = Workspace::new(root.clone());
        let mut manager = PermissionManager::new(PermissionRules::default());
        let first = prepared(
            ToolCapabilities::COMMAND,
            ToolPermissionSpec::default(),
            json!({ "path": "one" }),
        );
        let second = prepared(
            ToolCapabilities::COMMAND,
            ToolPermissionSpec::default(),
            json!({ "path": "two" }),
        );
        manager.remember_session_grant(&first);
        assert_eq!(
            manager.evaluate(&first, &workspace),
            PermissionDecision::Allow
        );
        assert!(matches!(
            manager.evaluate(&second, &workspace),
            PermissionDecision::Ask { .. }
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn name_scoped_session_grant_covers_changing_arguments_but_not_always_ask() {
        // MCP 远端工具:参数每次都不同,声明按名授权后一次审批覆盖同名后续调用。
        let root = temp_root("name-grant");
        let workspace = Workspace::new(root.clone());
        let mut manager = PermissionManager::new(PermissionRules::default());
        let by_name = ToolPermissionSpec {
            session_grant_by_name: true,
            ..ToolPermissionSpec::default()
        };
        let first = prepared(
            ToolCapabilities::MUTATION,
            by_name.clone(),
            json!({ "path": "one" }),
        );
        let second = prepared(
            ToolCapabilities::MUTATION,
            by_name,
            json!({ "path": "two" }),
        );
        assert!(matches!(
            manager.evaluate(&first, &workspace),
            PermissionDecision::Ask { scopes, .. } if scopes.contains(&ApprovalScope::Session)
        ));
        manager.remember_session_grant(&first);
        assert_eq!(
            manager.evaluate(&second, &workspace),
            PermissionDecision::Allow
        );

        // always_ask 优先:即使声明按名授权,高危边界仍然只有 Once,
        // session grant 不生效。
        let forced = ToolPermissionSpec {
            always_ask: true,
            session_grant_by_name: true,
            ..ToolPermissionSpec::default()
        };
        let call = prepared(
            ToolCapabilities::MUTATION,
            forced,
            json!({ "path": "three" }),
        );
        manager.remember_session_grant(&call);
        assert!(matches!(
            manager.evaluate(&call, &workspace),
            PermissionDecision::Ask { scopes, .. } if scopes == vec![ApprovalScope::Once]
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn opaque_side_effect_is_forced_once_even_when_all_rules_allow() {
        let root = temp_root("forced-once");
        let workspace = Workspace::new(root.clone());
        let mut manager = PermissionManager::new(PermissionRules {
            workspace_read: PermissionRule::Allow,
            workspace_write: PermissionRule::Allow,
            outside_workspace: PermissionRule::Allow,
            opaque_side_effect: PermissionRule::Allow,
        });
        let call = prepared(
            ToolCapabilities::COMMAND,
            ToolPermissionSpec::opaque_side_effect(&[]),
            json!({ "path": "same" }),
        );
        manager.remember_session_grant(&call);
        assert!(matches!(
            manager.evaluate(&call, &workspace),
            PermissionDecision::Ask {
                scopes,
                ..
            } if scopes == vec![ApprovalScope::Once]
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn dangerous_command_is_forced_once_but_ordinary_command_obeys_allow() {
        let root = temp_root("command-boundary");
        let workspace = Workspace::new(root.clone());
        let manager = PermissionManager::new(PermissionRules {
            workspace_read: PermissionRule::Allow,
            workspace_write: PermissionRule::Allow,
            outside_workspace: PermissionRule::Allow,
            opaque_side_effect: PermissionRule::Allow,
        });
        let permission =
            ToolPermissionSpec::command("path", None, crate::tools::CommandSyntax::PowerShell);
        let safe = prepared(
            ToolCapabilities::COMMAND,
            permission.clone(),
            json!({ "path": "Get-ChildItem" }),
        );
        assert_eq!(
            manager.evaluate(&safe, &workspace),
            PermissionDecision::Allow
        );

        let dangerous = prepared(
            ToolCapabilities::COMMAND,
            permission,
            json!({ "path": "Get-ChildItem | Remove-Item -Recurse" }),
        );
        assert!(matches!(
            manager.evaluate(&dangerous, &workspace),
            PermissionDecision::Ask {
                scopes,
                details: ApprovalDetails {
                    command: Some(_),
                    cwd: Some(_),
                    ..
                },
                ..
            } if scopes == vec![ApprovalScope::Once]
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn nonexistent_target_uses_canonical_existing_parent() {
        let root = temp_root("link-root");
        let outside = temp_root("link-outside");
        let link = root.join("escape");
        if !create_directory_link(&outside, &link) {
            let _ = std::fs::remove_dir_all(root);
            let _ = std::fs::remove_dir_all(outside);
            return;
        }
        let assessment = assess_path(&Workspace::new(root.clone()), "escape/new/file.txt").unwrap();
        assert!(!assessment.within_workspace);
        assert!(assessment
            .canonical
            .starts_with(std::fs::canonicalize(&outside).unwrap()));
        let _ = std::fs::remove_dir(link);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[cfg(windows)]
    #[test]
    fn windows_device_paths_are_hard_denied() {
        let root = temp_root("device");
        let workspace = Workspace::new(root.clone());
        assert!(assess_path(&workspace, r"\\.\PhysicalDrive0").is_err());
        assert!(assess_path(&workspace, "NUL.txt").is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    fn create_directory_link(target: &Path, link: &Path) -> bool {
        std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    #[cfg(unix)]
    fn create_directory_link(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
}
