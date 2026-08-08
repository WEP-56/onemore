import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useStore } from "@/app/store";
import type { FileTreeNode } from "@/app/types";
import {
  Folder,
  File as FileIcon,
  ChevronDown,
  ChevronRight,
  GitBranch,
  ArrowUp,
  ArrowDown,
  CircleDot,
  Plus,
  Minus,
  FileWarning,
} from "lucide-react";
import { cn } from "@/lib/utils";

export default function RightPanel() {
  const [tab, setTab] = useState<"files" | "git" | "plan">("files");
  const conn = useStore((s) => s.conn);

  if (conn === "disconnected") return <div className="w-0 overflow-hidden" />;

  return (
    <aside
      className="flex w-72 shrink-0 flex-col overflow-hidden"
      style={{ background: "var(--surface-right-panel)", borderLeft: "1px solid var(--border-subtle)" }}
    >
      <div className="flex h-10 shrink-0" style={{ borderBottom: "1px solid var(--border-subtle)" }}>
        {(["files", "git", "plan"] as const).map((t) => (
          <button
            key={t}
            type="button"
            className={cn(
              "flex-1 border-b-2 text-xs transition-colors",
              tab === t
                ? "text-[var(--text-strong)]"
                : "text-[var(--text-faint)] hover:text-[var(--text-muted)]",
            )}
            style={{ borderBottomColor: tab === t ? "var(--status-success)" : "transparent" }}
            onClick={() => setTab(t)}
          >
            {t === "files" ? "文件" : t === "git" ? "Git" : "计划"}
          </button>
        ))}
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        {tab === "files" && <FileTree />}
        {tab === "git" && <GitPanel />}
        {tab === "plan" && <PlanPanel />}
      </div>
    </aside>
  );
}

function FileTree() {
  const activeWorkspace = useStore((s) => s.activeWorkspace);
  const [tree, setTree] = useState<FileTreeNode[]>([]);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!activeWorkspace) return;
    setLoading(true);
    invoke<FileTreeNode[]>("get_file_tree", { workspace: activeWorkspace, maxDepth: 3 })
      .then(setTree)
      .catch(() => setTree([]))
      .finally(() => setLoading(false));
  }, [activeWorkspace]);

  if (loading) return <div className="p-4 text-center text-[13px] text-[var(--text-faint)]">加载中…</div>;
  if (tree.length === 0) return <div className="p-4 text-center text-[13px] text-[var(--text-faint)]">无文件</div>;

  return (
    <div className="py-1">
      {tree.map((n) => <TreeNode key={n.path} node={n} depth={0} />)}
    </div>
  );
}

function TreeNode({ node, depth }: { node: FileTreeNode; depth: number }) {
  const [expanded, setExpanded] = useState(depth < 1);

  if (!node.is_dir) {
    return (
      <div className="flex items-center gap-1 px-2 py-0.5 text-[13px] text-[var(--text-muted)]" style={{ paddingLeft: depth * 14 + 8 }}>
        <FileIcon size={13} className="shrink-0 text-[var(--text-faint)]" />
        <span className="truncate">{node.name}</span>
      </div>
    );
  }

  return (
    <div>
      <button
        type="button"
        className="flex w-full items-center gap-1 px-2 py-0.5 text-left text-[13px] text-[var(--text-strong)] transition-colors hover:bg-[var(--surface-hover)]"
        style={{ paddingLeft: depth * 14 + 4 }}
        onClick={() => setExpanded(!expanded)}
      >
        {expanded ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
        <Folder size={13} className="shrink-0" style={{ color: "var(--status-success)" }} />
        <span className="truncate font-medium">{node.name}</span>
      </button>
      {expanded && node.children.map((c) => <TreeNode key={c.path} node={c} depth={depth + 1} />)}
    </div>
  );
}

function GitPanel() {
  const git = useStore((s) => s.gitStatus);
  const activeWorkspace = useStore((s) => s.activeWorkspace);
  const loadGitStatus = useStore((s) => s.loadGitStatus);

  useEffect(() => {
    if (activeWorkspace) void loadGitStatus(activeWorkspace);
  }, [activeWorkspace, loadGitStatus]);

  if (!git) return <div className="p-4 text-center text-[13px] text-[var(--text-faint)]">加载中…</div>;
  if (!git.is_repo) return <div className="p-4 text-center text-[13px] text-[var(--text-faint)]">此工作区不是 Git 仓库</div>;

  return (
    <div>
      <div className="flex items-center gap-1.5 px-3 py-2.5 text-[13px]" style={{ borderBottom: "1px solid var(--border-subtle)" }}>
        <GitBranch size={14} style={{ color: "var(--git-branch)" }} />
        <span className="mono font-semibold">{git.branch}</span>
        {git.ahead > 0 && <span className="flex items-center gap-0.5 text-[11px] text-[var(--text-faint)]"><ArrowUp size={11} />{git.ahead}</span>}
        {git.behind > 0 && <span className="flex items-center gap-0.5 text-[11px] text-[var(--text-faint)]"><ArrowDown size={11} />{git.behind}</span>}
        <button type="button" className="ml-auto flex h-5 w-5 items-center justify-center rounded text-[var(--text-faint)] hover:bg-[var(--surface-hover)]" onClick={() => activeWorkspace && void loadGitStatus(activeWorkspace)}>
          <CircleDot size={12} />
        </button>
      </div>
      {git.files.length === 0 ? (
        <div className="p-4 text-center text-[13px] text-[var(--text-faint)]">工作区干净</div>
      ) : (
        <div className="py-1">
          {git.files.map((f) => {
            const color =
              f.status === "added" ? "var(--git-status-added)" :
              f.status === "deleted" ? "var(--git-status-deleted)" :
              f.status === "modified" ? "var(--git-status-modified)" :
              f.status === "renamed" ? "var(--git-status-renamed)" :
              f.status === "conflicted" ? "var(--git-status-conflict)" :
              "var(--text-faint)";
            return (
              <div key={f.path} className="flex items-center gap-1.5 px-3 py-1 text-xs hover:bg-[var(--surface-hover)]">
                <span className="flex shrink-0 items-center" style={{ color }}>
                  {f.status === "added" && <Plus size={12} />}
                  {f.status === "deleted" && <Minus size={12} />}
                  {(f.status === "modified" || f.status === "renamed" || f.status === "conflicted") && <FileWarning size={12} />}
                  {f.status === "untracked" && <CircleDot size={12} />}
                </span>
                <span className="mono flex-1 truncate text-[var(--text-muted)]" title={f.path}>{f.path}</span>
                <span className="shrink-0 rounded-full px-1.5 py-0.5 text-[10px]" style={{ color, background: `${color}1a` }}>
                  {f.staged ? "已暂存" : f.status}
                </span>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

function PlanPanel() {
  const plan = useStore((s) => s.snapshot?.plan);
  const phase = useStore((s) => s.snapshot?.phase ?? "idle");

  if (!plan || plan.items.length === 0) {
    return (
      <div className="p-4 text-center text-[13px] text-[var(--text-faint)]">
        {phase === "idle" ? "暂无计划。发送消息后自动生成。" : "等待计划生成…"}
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-2.5 p-3">
      {plan.explanation && (
        <div className="whitespace-pre-wrap rounded-md px-2.5 py-2 text-[13px] text-[var(--text-muted)]" style={{ background: "var(--surface-card)", border: "1px solid var(--border-subtle)" }}>
          {plan.explanation}
        </div>
      )}
      <div className="flex flex-col gap-1">
        {plan.items.map((item) => (
          <div
            key={item.id}
            className={cn(
              "flex items-start gap-2 rounded-md px-2 py-1.5 text-[13px]",
              item.status === "in_progress" && "bg-[rgba(120,235,190,0.08)]",
            )}
            style={{
              color: item.status === "completed" ? "var(--text-faint)" : item.status === "in_progress" ? "var(--text-strong)" : "var(--text-muted)",
            }}
          >
            <span className="mt-0.5 shrink-0" style={{ color: item.status === "in_progress" ? "var(--status-success)" : "var(--text-faint)" }}>
              {item.status === "completed" && <CheckIcon />}
              {item.status === "in_progress" && <LoaderIcon />}
              {item.status === "pending" && <CirclePending />}
            </span>
            <span className={cn(item.status === "completed" && "line-through")}>{item.text}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

function CheckIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
      <path d="M22 11.08V12a10 10 0 1 1-5.93-9.14" />
      <polyline points="22 4 12 14.01 9 11.01" />
    </svg>
  );
}
function LoaderIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" className="spin">
      <line x1="12" y1="2" x2="12" y2="6" />
      <line x1="12" y1="18" x2="12" y2="22" />
      <line x1="4.93" y1="4.93" x2="7.76" y2="7.76" />
      <line x1="16.24" y1="16.24" x2="19.07" y2="19.07" />
      <line x1="2" y1="12" x2="6" y2="12" />
      <line x1="18" y1="12" x2="22" y2="12" />
    </svg>
  );
}
function CirclePending() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
      <circle cx="12" cy="12" r="10" />
    </svg>
  );
}
