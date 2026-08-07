//! Workspace-owned project instructions loaded from a root `AGENTS.md`.

use std::path::PathBuf;

use super::{ContextProvider, PromptContext};
use crate::workspace::Workspace;

const FILE_NAME: &str = "AGENTS.md";

/// A startup-frozen project instruction file.
#[derive(Debug)]
pub struct ProjectInstructions {
    path: PathBuf,
    content: String,
}

impl ProjectInstructions {
    /// Load the workspace-root `AGENTS.md` once. Missing and empty files add no
    /// prompt section; malformed or unreadable files return a non-fatal diagnostic.
    pub fn discover(workspace: &Workspace) -> Result<Option<Self>, String> {
        let path = workspace.root().join(FILE_NAME);
        let exists = path
            .try_exists()
            .map_err(|error| format!("检查 {} 失败: {}", path.display(), error))?;
        if !exists {
            return Ok(None);
        }

        let content = workspace.read_text(&path)?;
        if content.trim().is_empty() {
            return Ok(None);
        }

        Ok(Some(Self { path, content }))
    }

    fn render(&self) -> String {
        format!(
            "<project_context>\n\nProject-specific instructions and guidelines:\n\n\
             <project_instructions path=\"{}\">\n{}\n</project_instructions>\n\n\
             </project_context>",
            escape_xml_attribute(&self.path.display().to_string()),
            self.content
        )
    }
}

impl ContextProvider for ProjectInstructions {
    fn name(&self) -> &'static str {
        "project_instructions"
    }

    fn contribute(&self, prompt: &mut PromptContext, _workspace: &Workspace) {
        prompt.system_sections.push(self.render());
    }
}

fn escape_xml_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("onemore-agents-{label}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn missing_or_empty_file_adds_no_context() {
        let root = temp_root("empty");
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("nested").join(FILE_NAME), "nested only").unwrap();
        let workspace = Workspace::new(root.clone());
        assert!(ProjectInstructions::discover(&workspace).unwrap().is_none());

        fs::write(root.join(FILE_NAME), " \n\t").unwrap();
        assert!(ProjectInstructions::discover(&workspace).unwrap().is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn discovered_instructions_are_frozen_and_marked() {
        let root = temp_root("frozen&marked");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(FILE_NAME), "  Keep the public API small.\n").unwrap();
        let workspace = Workspace::new(root.clone());
        let provider = ProjectInstructions::discover(&workspace)
            .unwrap()
            .expect("AGENTS.md should be discovered");

        fs::write(root.join(FILE_NAME), "changed after startup").unwrap();
        let mut prompt = PromptContext::default();
        provider.contribute(&mut prompt, &workspace);
        let rendered = prompt.system_text();

        assert!(rendered.starts_with("<project_context>"));
        assert!(rendered.contains("<project_instructions path=\""));
        assert!(rendered.contains("frozen&amp;marked"));
        assert!(rendered.contains("\n  Keep the public API small.\n"));
        assert!(!rendered.contains("changed after startup"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn unreadable_text_returns_a_diagnostic() {
        let root = temp_root("invalid-utf8");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(FILE_NAME), [0xff, 0xfe]).unwrap();
        let error = ProjectInstructions::discover(&Workspace::new(root.clone())).unwrap_err();
        assert!(error.contains(FILE_NAME));
        let _ = fs::remove_dir_all(root);
    }
}
