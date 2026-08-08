import { useMemo } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useStore } from "@/app/store";
import { normalizeWorkspace, relativeTime } from "@/app/util";
import {
  Plus,
  Search,
  Settings,
  MessageSquare,
  Folder,
  Trash2,
  RefreshCw,
} from "lucide-react";
import { cn } from "@/lib/utils";

export default function Sidebar() {
  const workspaces = useStore((s) => s.workspaces);
  const activeWorkspace = useStore((s) => s.activeWorkspace);
  const sessions = useStore((s) => s.sessions);
  const searchQuery = useStore((s) => s.searchQuery);
  const setSearchQuery = useStore((s) => s.setSearchQuery);
  const addWorkspace = useStore((s) => s.addWorkspace);
  const removeWorkspace = useStore((s) => s.removeWorkspace);
  const selectWorkspace = useStore((s) => s.selectWorkspace);
  const connect = useStore((s) => s.connect);
  const loadSessions = useStore((s) => s.loadSessions);
  const loadSession = useStore((s) => s.loadSession);
  const setSettingsOpen = useStore((s) => s.setSettingsOpen);
  const clearConversation = useStore((s) => s.clearConversation);
  const conn = useStore((s) => s.conn);

  const handleAddWorkspace = async () => {
    const dir = await open({ directory: true, title: "选择工作区目录" });
    if (typeof dir === "string") await addWorkspace(dir);
  };

  const handleSelectWorkspace = async (path: string) => {
    await selectWorkspace(path);
    await connect(path);
    await loadSessions();
  };

  const filteredSessions = useMemo(() => {
    const q = searchQuery.trim().toLowerCase();
    let list = sessions;
    if (!q) {
      if (activeWorkspace) {
        const norm = normalizeWorkspace(activeWorkspace).toLowerCase();
        list = sessions.filter(
          (s) => normalizeWorkspace(s.workspace).toLowerCase() === norm,
        );
      }
    } else {
      list = sessions.filter(
        (s) =>
          s.title.toLowerCase().includes(q) ||
          normalizeWorkspace(s.workspace).toLowerCase().includes(q),
      );
    }
    return list;
  }, [sessions, searchQuery, activeWorkspace]);

  return (
    <aside
      className="flex w-60 shrink-0 flex-col border-r overflow-hidden"
      style={{ background: "var(--surface-sidebar)", borderColor: "var(--border-subtle)" }}
    >
      {/* Header */}
      <div
        className="flex h-11 shrink-0 items-center justify-between px-3"
        style={{ borderBottom: "1px solid var(--border-subtle)" }}
      >
        <div className="flex items-center gap-2">
          <span
            className="inline-block h-2.5 w-2.5 rounded-full"
            style={{ background: "var(--status-success)", boxShadow: "0 0 8px var(--status-success)" }}
          />
          <span className="text-[15px] font-semibold tracking-tight">OnemoreGui</span>
        </div>
        <button
          type="button"
          className="flex h-7 w-7 items-center justify-center rounded transition-colors hover:bg-[var(--surface-hover)]"
          title="设置"
          onClick={() => setSettingsOpen(true)}
        >
          <Settings size={15} className="text-[var(--text-muted)]" />
        </button>
      </div>

      {/* Workspaces */}
      <div className="flex flex-col gap-1 px-2 pt-2">
        <div className="flex items-center justify-between px-1 pb-1">
          <span className="text-[11px] font-semibold uppercase tracking-wider text-[var(--text-faint)]">
            工作区
          </span>
          <button
            type="button"
            className="flex h-5 w-5 items-center justify-center rounded text-[var(--text-faint)] transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--text-strong)]"
            title="添加工作区"
            onClick={() => void handleAddWorkspace()}
          >
            <Plus size={14} />
          </button>
        </div>
        <div className="flex flex-col gap-0.5">
          {workspaces.length === 0 && (
            <div className="px-2 py-1.5 text-xs text-[var(--text-faint)]">点击 + 添加工作区</div>
          )}
          {workspaces.map((w) => (
            <div
              key={w.path}
              className={cn(
                "group flex items-center rounded-md transition-colors",
                activeWorkspace === w.path
                  ? "bg-[var(--surface-hover)]"
                  : "hover:bg-[var(--surface-hover)]",
              )}
            >
              <button
                type="button"
                className="flex flex-1 items-center gap-2 overflow-hidden px-2 py-1.5 text-left"
                title={w.path}
                onClick={() => void handleSelectWorkspace(w.path)}
              >
                <Folder size={14} className="shrink-0 text-[var(--text-faint)]" />
                <span
                  className={cn(
                    "truncate text-[13px]",
                    activeWorkspace === w.path
                      ? "text-[var(--text-strong)] font-medium"
                      : "text-[var(--text-muted)]",
                  )}
                >
                  {w.label}
                </span>
              </button>
              <button
                type="button"
                className="mr-1 flex h-5 w-5 shrink-0 items-center justify-center rounded text-[var(--text-faint)] opacity-0 transition-opacity hover:text-[var(--status-error)] group-hover:opacity-100"
                title="移除"
                onClick={() => void removeWorkspace(w.path)}
              >
                <Trash2 size={12} />
              </button>
            </div>
          ))}
        </div>
      </div>

      {/* Search */}
      <div
        className="flex items-center gap-2 px-3 py-2"
        style={{
          borderTop: "1px solid var(--border-subtle)",
          borderBottom: "1px solid var(--border-subtle)",
        }}
      >
        <Search size={14} className="shrink-0 text-[var(--text-faint)]" />
        <input
          className="flex-1 border-none bg-transparent text-[13px] outline-none placeholder:text-[var(--text-faint)]"
          placeholder="搜索会话…"
          value={searchQuery}
          onChange={(e) => setSearchQuery(e.target.value)}
          spellCheck={false}
        />
      </div>

      {/* Sessions */}
      <div className="flex min-h-0 flex-1 flex-col">
        <div className="flex items-center justify-between px-3 py-1.5">
          <span className="text-[11px] font-semibold uppercase tracking-wider text-[var(--text-faint)]">
            会话
          </span>
          <button
            type="button"
            className="flex h-5 w-5 items-center justify-center rounded text-[var(--text-faint)] transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--text-strong)]"
            title="刷新"
            onClick={() => void loadSessions()}
          >
            <RefreshCw size={12} />
          </button>
        </div>
        <div className="flex-1 overflow-y-auto px-2 pb-2">
          {filteredSessions.length === 0 && (
            <div className="px-2 py-3 text-center text-xs text-[var(--text-faint)]">
              {searchQuery ? "无匹配会话" : "暂无会话"}
            </div>
          )}
          {filteredSessions.map((s) => (
            <button
              key={s.id}
              type="button"
              className="flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-[var(--surface-hover)]"
              title={s.title}
              onClick={() => void loadSession(s.id)}
            >
              <MessageSquare size={13} className="mt-0.5 shrink-0 text-[var(--text-faint)]" />
              <div className="flex min-w-0 flex-col gap-0.5">
                <span className="truncate text-[13px] text-[var(--text-primary)]">{s.title || "（无标题）"}</span>
                <span className="text-[11px] text-[var(--text-faint)]">
                  {s.message_count} msg · {relativeTime(s.updated_at)}
                </span>
              </div>
            </button>
          ))}
        </div>
      </div>

      {/* Footer */}
      <div
        className="px-3 py-2.5"
        style={{ borderTop: "1px solid var(--border-subtle)" }}
      >
        <button
          type="button"
          className="flex w-full items-center justify-center gap-1.5 rounded-md py-2 text-[13px] font-medium text-black transition-colors disabled:opacity-40"
          style={{ background: conn === "connected" ? "var(--primary)" : "var(--surface-control)" }}
          disabled={conn !== "connected"}
          onClick={() => void clearConversation()}
        >
          <Plus size={15} /> 新建会话
        </button>
      </div>
    </aside>
  );
}
