import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useStore } from "@/app/store";
import type { FileTreeNode, GitFileStatus } from "@/app/types";
import {
  Folder,
  File as FileIcon,
  GitBranch,
  ArrowUp,
  ArrowDown,
  RefreshCw,
  Plus,
  Minus,
  FileWarning,
  CircleDot,
  ListChecks,
  FileDiff,
} from "lucide-react";
import { cn } from "@/lib/utils";

type PanelTabId = "files" | "git" | "plan";

const TABS: { id: PanelTabId; label: string; icon: React.ReactNode }[] = [
  { id: "files", label: "文件", icon: <Folder /> },
  { id: "git", label: "Git", icon: <GitBranch /> },
  { id: "plan", label: "计划", icon: <ListChecks /> },
];

export default function RightPanel() {
  const [tab, setTab] = useState<PanelTabId>("files");
  const conn = useStore((s) => s.conn);

  if (conn === "disconnected") return <div className="w-0 overflow-hidden" />;

  return (
    <>
      <div className="right-panel-toolbar" role="tablist" aria-label="面板">
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            aria-selected={tab === t.id}
            className={cn("panel-tab", tab === t.id && "is-active")}
            title={t.label}
            onClick={() => setTab(t.id)}
          >
            {t.icon}
          </button>
        ))}
      </div>
      <div className="right-panel-body">
        {tab === "files" && <FileTree />}
        {tab === "git" && <GitPanel />}
        {tab === "plan" && <PlanPanel />}
      </div>
    </>
  );
}

/* ── 文件树 ── */
function FileTree() {
  const activeWorkspace = useStore((s) => s.activeWorkspace);
  const [tree, setTree] = useState<FileTreeNode[]>([]);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!activeWorkspace) return;
    setLoading(true);
    invoke<FileTreeNode[]>("get_file_tree", { workspace: activeWorkspace, maxDepth: 4 })
      .then(setTree)
      .catch(() => setTree([]))
      .finally(() => setLoading(false));
  }, [activeWorkspace]);

  if (loading) return <div className="p-4 text-center text-[13px] text-[var(--text-faint)]">加载中…</div>;
  if (tree.length === 0) return <div className="p-4 text-center text-[13px] text-[var(--text-faint)]">无文件</div>;

  return (
    <div className="py-1.5">
      {tree.map((n) => <TreeNode key={n.path} node={n} depth={0} />)}
    </div>
  );
}

function TreeNode({ node, depth }: { node: FileTreeNode; depth: number }) {
  const [expanded, setExpanded] = useState(depth < 1);

  if (!node.is_dir) {
    return (
      <div className="file-tree-row" style={{ paddingLeft: depth * 14 + 10 }}>
        <span className="tree-chevron" />
        <FileIcon size={13} className="tree-icon" />
        <span className="file-tree-name">{node.name}</span>
      </div>
    );
  }

  return (
    <div>
      <div
        className="file-tree-row dir"
        style={{ paddingLeft: depth * 14 + 8 }}
        onClick={() => setExpanded(!expanded)}
        role="button"
        tabIndex={0}
      >
        <ChevronIcon open={expanded} />
        <Folder size={13} className="tree-icon" />
        <span className="file-tree-name">{node.name}</span>
      </div>
      {expanded && node.children.map((c) => <TreeNode key={c.path} node={c} depth={depth + 1} />)}
    </div>
  );
}

function ChevronIcon({ open }: { open: boolean }) {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      className={cn("tree-chevron", open && "is-open")}
    >
      <path d="m9 18 6-6-6-6" />
    </svg>
  );
}

/* ── Git 面板 ── */
function GitPanel() {
  const git = useStore((s) => s.gitStatus);
  const activeWorkspace = useStore((s) => s.activeWorkspace);
  const loadGitStatus = useStore((s) => s.loadGitStatus);

  useEffect(() => {
    if (activeWorkspace) void loadGitStatus(activeWorkspace);
  }, [activeWorkspace, loadGitStatus]);

  const { staged, unstaged } = useMemo(() => {
    const s: GitFileStatus[] = [];
    const u: GitFileStatus[] = [];
    for (const f of git?.files ?? []) {
      if (f.staged) s.push(f);
      else u.push(f);
    }
    return { staged: s, unstaged: u };
  }, [git]);

  if (!git) return <div className="p-4 text-center text-[13px] text-[var(--text-faint)]">加载中…</div>;
  if (!git.is_repo) return <div className="p-4 text-center text-[13px] text-[var(--text-faint)]">此工作区不是 Git 仓库</div>;

  return (
    <div className="git-panel">
      <div className="git-panel-header">
        <GitBranch size={14} style={{ color: "var(--git-branch)" }} />
        <span className="git-branch-name" title={git.branch}>{git.branch}</span>
        {git.ahead > 0 && (
          <span className="git-ahead-behind" title={`领先 ${git.ahead}`}>
            <ArrowUp size={11} />{git.ahead}
          </span>
        )}
        {git.behind > 0 && (
          <span className="git-ahead-behind" title={`落后 ${git.behind}`}>
            <ArrowDown size={11} />{git.behind}
          </span>
        )}
        <button
          type="button"
          className="ml-auto flex h-6 w-6 items-center justify-center rounded text-[var(--text-faint)] hover:bg-[var(--surface-hover)] hover:text-[var(--text-strong)]"
          title="刷新"
          onClick={() => activeWorkspace && void loadGitStatus(activeWorkspace)}
        >
          <RefreshCw size={12} />
        </button>
      </div>
      <div className="git-panel-body">
        {git.files.length === 0 ? (
          <div className="git-clean">工作区干净 ✨</div>
        ) : (
          <>
            {staged.length > 0 && (
              <>
                <div className="git-section-label">已暂存 ({staged.length})</div>
                {staged.map((f) => <GitFileRow key={f.path} file={f} />)}
              </>
            )}
            {unstaged.length > 0 && (
              <>
                <div className="git-section-label">未暂存 ({unstaged.length})</div>
                {unstaged.map((f) => <GitFileRow key={f.path} file={f} />)}
              </>
            )}
          </>
        )}
      </div>
    </div>
  );
}

const STATUS_META: Record<string, { color: string; icon: React.ReactNode; label: string }> = {
  added: { color: "var(--git-status-added)", icon: <Plus size={12} />, label: "新增" },
  deleted: { color: "var(--git-status-deleted)", icon: <Minus size={12} />, label: "删除" },
  modified: { color: "var(--git-status-modified)", icon: <FileWarning size={12} />, label: "修改" },
  renamed: { color: "var(--git-status-renamed)", icon: <FileDiff size={12} />, label: "重命名" },
  conflicted: { color: "var(--git-status-conflict)", icon: <FileWarning size={12} />, label: "冲突" },
  untracked: { color: "var(--text-faint)", icon: <CircleDot size={12} />, label: "未跟踪" },
};

function GitFileRow({ file }: { file: GitFileStatus }) {
  const meta = STATUS_META[file.status] ?? { color: "var(--text-faint)", icon: <CircleDot size={12} />, label: file.status };
  return (
    <div className="git-file-row" title={file.path}>
      <span className="git-file-status-icon" style={{ color: meta.color }}>
        {meta.icon}
      </span>
      <span className="git-file-path">{file.path}</span>
      <span className="git-file-badge" style={{ color: meta.color, background: `${meta.color}1a` }}>
        {meta.label}
      </span>
    </div>
  );
}

/* ── 计划面板 ── */
function PlanPanel() {
  const plan = useStore((s) => s.snapshot?.plan);
  const phase = useStore((s) => s.snapshot?.phase ?? "idle");
  const items = plan?.items ?? [];
  const completed = items.filter((i) => i.status === "completed").length;
  const processing = phase !== "idle";

  return (
    <aside className="plan-panel">
      <div className="plan-header">
        <span>计划</span>
        {items.length > 0 && <span className="plan-progress">{completed}/{items.length}</span>}
      </div>
      {plan?.explanation && <div className="plan-explanation">{plan.explanation}</div>}
      {items.length === 0 ? (
        <div className="plan-empty">
          {processing ? "等待计划生成…" : "暂无计划。发送消息后自动生成。"}
        </div>
      ) : (
        <ol className="plan-list">
          {items.map((item) => (
            <li key={item.id} className={cn("plan-step", item.status)}>
              <span className="plan-step-status">
                {item.status === "completed" ? "[x]" : item.status === "in_progress" ? "[>]" : "[ ]"}
              </span>
              <span className="plan-step-text">{item.text}</span>
            </li>
          ))}
        </ol>
      )}
    </aside>
  );
}
