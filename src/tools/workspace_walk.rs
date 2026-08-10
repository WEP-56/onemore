use std::path::{Path, PathBuf};

use ignore::{DirEntry, Walk, WalkBuilder};

/// Generated and metadata directories do not belong in general workspace
/// discovery. They can still be inspected deliberately with `list_dir`.
pub(crate) const SKIP_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "__pycache__",
    ".venv",
    ".mypy_cache",
    ".pytest_cache",
];

pub(crate) fn walk(root: &Path) -> Walk {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .ignore(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .follow_links(false)
        .sort_by_file_path(|left, right| left.cmp(right))
        .filter_entry(keep_entry);
    builder.build()
}

fn keep_entry(entry: &DirEntry) -> bool {
    !entry
        .file_type()
        .is_some_and(|file_type| file_type.is_dir())
        || !SKIP_DIRS.iter().any(|name| entry.file_name() == *name)
}

pub(crate) fn relative_display(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}
