//! Git 状态查询与文件树预览。

use std::path::Path;

use serde::Serialize;

use crate::error::GuiError;

#[derive(Debug, Clone, Serialize)]
pub struct GitFileStatus {
    pub path: String,
    pub status: String, // "modified", "added", "deleted", "renamed", "untracked", "conflicted"
    pub staged: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitStatus {
    pub branch: String,
    pub ahead: usize,
    pub behind: usize,
    pub files: Vec<GitFileStatus>,
    pub is_repo: bool,
}

/// 查询工作区 git 状态。
pub fn git_status(workspace: &str) -> Result<GitStatus, GuiError> {
    let repo_path = Path::new(workspace);
    let repo = match git2::Repository::open(repo_path) {
        Ok(r) => r,
        Err(_) => {
            return Ok(GitStatus {
                branch: String::new(),
                ahead: 0,
                behind: 0,
                files: Vec::new(),
                is_repo: false,
            });
        }
    };

    // 分支名
    let branch = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(|s| s.to_string()))
        .unwrap_or_default();

    // ahead / behind
    let (ahead, behind) = {
        let head = repo.head().ok();
        let local_oid = head.as_ref().and_then(|h| h.target());
        let upstream = repo
            .find_branch(
                &format!("origin/{branch}"),
                git2::BranchType::Remote,
            )
            .ok()
            .and_then(|b| b.get().target());
        match (local_oid, upstream) {
            (Some(local), Some(remote)) => repo
                .graph_ahead_behind(local, remote)
                .unwrap_or((0, 0)),
            _ => (0, 0),
        }
    };

    // 文件状态
    let mut files = Vec::new();
    let statuses = repo
        .statuses(Some(
            git2::StatusOptions::new()
                .include_untracked(true)
                .recurse_untracked_dirs(true),
        ))
        .map_err(|e| GuiError::new("git_status", e.to_string()))?;

    for entry in statuses.iter() {
        let path = entry.path().unwrap_or("").to_string();
        if path.is_empty() {
            continue;
        }
        let s = entry.status();
        let (status_str, staged) = status_to_string(s, &entry);
        files.push(GitFileStatus {
            path,
            status: status_str,
            staged,
        });
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(GitStatus {
        branch,
        ahead,
        behind,
        files,
        is_repo: true,
    })
}

fn status_to_string(s: git2::Status, entry: &git2::StatusEntry) -> (String, bool) {
    let staged = matches!(
        s,
        git2::Status::INDEX_NEW
            | git2::Status::INDEX_MODIFIED
            | git2::Status::INDEX_DELETED
            | git2::Status::INDEX_RENAMED
            | git2::Status::INDEX_TYPECHANGE
    );

    let status_str = if s.is_wt_new() || s == git2::Status::WT_NEW {
        "untracked".to_string()
    } else if s == git2::Status::CONFLICTED {
        "conflicted".to_string()
    } else if s == git2::Status::IGNORED {
        return (String::new(), false); // skip ignored
    } else if s == git2::Status::CURRENT {
        return (String::new(), false); // skip clean
    } else {
        let mut parts = Vec::new();
        if s.contains(git2::Status::INDEX_NEW) {
            parts.push("added");
        }
        if s.contains(git2::Status::INDEX_MODIFIED) || s.contains(git2::Status::WT_MODIFIED) {
            parts.push("modified");
        }
        if s.contains(git2::Status::INDEX_DELETED) || s.contains(git2::Status::WT_DELETED) {
            parts.push("deleted");
        }
        if s.contains(git2::Status::INDEX_RENAMED) {
            parts.push("renamed");
        }
        if parts.is_empty() {
            "modified".to_string()
        } else {
            parts[0].to_string()
        }
    };

    // 过滤 ignored
    if entry.status() == git2::Status::IGNORED {
        return (String::new(), false);
    }

    (status_str, staged)
}

#[derive(Debug, Clone, Serialize)]
pub struct FileTreeNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub children: Vec<FileTreeNode>,
}

/// 读取工作区文件树（使用 ignore crate 尊重 .gitignore）。
pub fn file_tree(workspace: &str, max_depth: usize) -> Result<Vec<FileTreeNode>, GuiError> {
    let root = Path::new(workspace);
    if !root.is_dir() {
        return Err(GuiError::new("invalid_workspace", "工作区目录不存在"));
    }
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .max_depth(Some(max_depth))
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .parents(false);

    let mut nodes = build_tree(root, &mut builder, max_depth)?;
    nodes.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    for n in &mut nodes {
        sort_recursive(n);
    }
    Ok(nodes)
}

fn build_tree(
    root: &Path,
    builder: &mut ignore::WalkBuilder,
    max_depth: usize,
) -> Result<Vec<FileTreeNode>, GuiError> {
    let walker = builder.build();
    let mut top_level: Vec<FileTreeNode> = Vec::new();

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path == root {
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(path);
        let depth = rel.components().count();
        if depth > max_depth {
            continue;
        }

        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let rel_path = rel.to_string_lossy().replace('\\', "/");

        insert_into_tree(&mut top_level, &rel_path, &name, is_dir, root);
    }

    Ok(top_level)
}

fn insert_into_tree(
    nodes: &mut Vec<FileTreeNode>,
    rel_path: &str,
    name: &str,
    is_dir: bool,
    _root: &Path,
) {
    let parts: Vec<&str> = rel_path.split('/').collect();
    if parts.len() == 1 {
        nodes.push(FileTreeNode {
            name: name.to_string(),
            path: rel_path.to_string(),
            is_dir,
            children: Vec::new(),
        });
        return;
    }

    let first = parts[0];
    let node = nodes
        .iter_mut()
        .find(|n| n.name == first && n.is_dir);
    if let Some(node) = node {
        let remaining = parts[1..].join("/");
        insert_into_tree(&mut node.children, &remaining, name, is_dir, _root);
    } else {
        // 中间目录不存在（ignore walker 可能跳过），创建占位
        let mut new_node = FileTreeNode {
            name: first.to_string(),
            path: first.to_string(),
            is_dir: true,
            children: Vec::new(),
        };
        let remaining = parts[1..].join("/");
        insert_into_tree(&mut new_node.children, &remaining, name, is_dir, _root);
        nodes.push(new_node);
    }
}

fn sort_recursive(node: &mut FileTreeNode) {
    node.children.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    for c in &mut node.children {
        sort_recursive(c);
    }
}
