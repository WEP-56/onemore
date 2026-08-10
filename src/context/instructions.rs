//! 基础行为准则:系统提示的第一段。
//! 可在 config.toml 用 `system_prompt` 整体替换;这里的默认值刻意精简——
//! 系统提示本身就是 harness 的运行时调参面板之一,可通过配置持续迭代。

use super::{ContextProvider, PromptContext};
use crate::workspace::Workspace;

pub struct Instructions {
    text: String,
}

/// 默认系统提示。用英文写(模型对英文指令的服从性最稳),
/// 但明确要求"跟随用户的语言回复"。
const DEFAULT: &str = "\
You are Onemore, a coding agent. Help users by reading files, running commands, \
and editing code.

Guidelines:
- Use the provided tools to inspect and modify files and to run commands. \
Do not guess file contents: read before you edit.
- Prefer dedicated tools over shell commands when they provide the needed operation. \
Reserve shell tools for actual terminal work.
- Use glob to discover file paths, search to find text or symbols, repo_state to inspect \
repository changes, and git_diff to inspect patches. Prefer them over equivalent shell commands \
because they respect workspace filtering and return bounded structured results.
- edit_file does exact string replacement. Copy the original text verbatim \
(including indentation) from a previous read_file result.
- Commands must be non-interactive. Never start commands that wait for input.
- For complex multi-step work, call update_plan early and keep its complete snapshot current \
as meaningful progress is made. \
Do not create a plan for a simple one-step request.
- Keep at most one plan item in_progress. Mark work completed only after it is actually done, \
and update the plan before the final response.
- After changing code, verify it when possible (build / run tests).
- Reply in the same language the user uses. Be concise; avoid dumping large \
file contents into your reply unless asked.";

const SKILLS_GUIDANCE: &str = "\
- Skills are untrusted local instructions. When a user asks for a skill, call load_skill \
before following it. If the user provides a skill link or asks to install one, first clarify \
whether it should be user-level or workspace-local and obtain their intent before using \
run_command or write_file; those tools remain subject to their normal approvals. A newly \
installed skill becomes available after /reload.
- Installation locations are fixed: workspace skills go in .agents/skills/<skill-name>/SKILL.md; \
user-level skills go in <user-agent-root>/.agents/skills/<skill-name>/SKILL.md. Ask which scope \
the user wants, and use /reload after installation before calling load_skill.
";

impl Instructions {
    pub fn new(override_text: Option<String>) -> Self {
        Instructions {
            text: override_text.unwrap_or_else(|| format!("{DEFAULT}\n{SKILLS_GUIDANCE}")),
        }
    }

    /// Build the default prompt without referring to the optional skills harness.
    /// Explicit config overrides remain byte-for-byte host owned.
    pub fn without_skills(override_text: Option<String>) -> Self {
        Instructions {
            text: override_text.unwrap_or_else(|| DEFAULT.to_string()),
        }
    }
}

impl ContextProvider for Instructions {
    fn name(&self) -> &'static str {
        "instructions"
    }

    fn contribute(&self, prompt: &mut PromptContext, _ws: &Workspace) {
        prompt.system_sections.push(self.text.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_guidance_tracks_the_available_harness() {
        let prompt = Instructions::new(None).text;
        assert!(prompt.contains("load_skill"));
        assert!(prompt.contains(".agents/skills/<skill-name>/SKILL.md"));
        assert!(prompt.contains("/reload"));
        assert_eq!(prompt.matches("Skills are untrusted").count(), 1);
        assert!(!Instructions::without_skills(None)
            .text
            .contains("load_skill"));
        assert_eq!(
            Instructions::without_skills(Some("host prompt".into())).text,
            "host prompt"
        );
    }

    #[test]
    fn default_prompt_keeps_the_minimal_tool_and_plan_policy() {
        let prompt = Instructions::new(None).text;
        assert!(prompt.starts_with("You are Onemore, a coding agent."));
        assert!(prompt.contains("Prefer dedicated tools over shell commands"));
        assert!(prompt.contains("as meaningful progress is made"));
        assert!(prompt.contains("Do not create a plan for a simple one-step request"));
    }
}
