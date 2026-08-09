// 会话:按工作区分类列出所有会话,支持删除、重命名。

import { useEffect, useMemo, useState } from "react";
import { useStore } from "@/app/store";
import { MessageSquare, Pencil, Trash2 } from "lucide-react";
import { normalizeWorkspace, relativeTime, workspaceKey } from "@/app/util";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

export default function SessionsSection() {
  const sessions = useStore((s) => s.sessions);
  const workspaces = useStore((s) => s.workspaces);
  const loadSessions = useStore((s) => s.loadSessions);
  const deleteSession = useStore((s) => s.deleteSession);
  const renameSession = useStore((s) => s.renameSession);
  const [query, setQuery] = useState("");
  const [renameTarget, setRenameTarget] = useState<{ id: string; title: string } | null>(null);
  const [renameValue, setRenameValue] = useState("");

  useEffect(() => {
    void loadSessions();
  }, [loadSessions]);

  const byWorkspace = useMemo(() => {
    const q = query.trim().toLowerCase();
    const map = new Map<string, typeof sessions>();
    for (const s of sessions) {
      if (q && !s.title.toLowerCase().includes(q) && !normalizeWorkspace(s.workspace).toLowerCase().includes(q)) continue;
      const key = workspaceKey(s.workspace);
      const list = map.get(key) ?? [];
      list.push(s);
      map.set(key, list);
    }
    for (const list of map.values()) list.sort((a, b) => b.updated_at - a.updated_at);
    return map;
  }, [sessions, query]);

  const workspaceLabel = (key: string, fallbackPath: string) =>
    workspaces.find((w) => workspaceKey(w.path) === key)?.label ?? normalizeWorkspace(fallbackPath).split(/[\\/]/).pop() ?? fallbackPath;

  const total = sessions.length;

  return (
    <div>
      <div className="settings-section">
        <div className="flex items-center justify-between gap-3">
          <div>
            <h3 className="settings-section-title" style={{ marginBottom: 2 }}>会话管理</h3>
            <p className="settings-section-desc">共 {total} 个会话,按工作区分类。删除操作不可恢复。</p>
          </div>
          <Input
            className="h-8 w-52"
            placeholder="搜索会话…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>

        {byWorkspace.size === 0 && (
          <div className="settings-empty">{query ? "无匹配会话" : "暂无会话"}</div>
        )}

        {[...byWorkspace.entries()].map(([key, list]) => (
          <div key={key} className="settings-workspace-group">
            <div className="settings-workspace-group-header">
              <MessageSquare size={12} />
              {workspaceLabel(key, list[0]?.workspace ?? key)}
              <span className="text-[10px] opacity-70">{list.length}</span>
            </div>
            <div className="settings-card" style={{ padding: 6 }}>
              {list.map((s) => (
                <div key={s.id} className="settings-session-row">
                  <span className="settings-session-title" title={s.title}>{s.title || "（无标题）"}</span>
                  <span className="settings-session-meta">{s.message_count} msg · {relativeTime(s.updated_at)}</span>
                  <span className="settings-session-actions">
                    <button
                      type="button"
                      className="sidebar-icon-btn"
                      title="重命名"
                      onClick={() => {
                        setRenameTarget({ id: s.id, title: s.title });
                        setRenameValue(s.title);
                      }}
                    >
                      <Pencil size={12} />
                    </button>
                    <button
                      type="button"
                      className="sidebar-icon-btn danger"
                      title="删除会话"
                      onClick={() => void deleteSession(s.id)}
                    >
                      <Trash2 size={12} />
                    </button>
                  </span>
                </div>
              ))}
            </div>
          </div>
        ))}
      </div>

      {renameTarget && (
        <div className="fixed inset-0 z-[120] flex items-center justify-center" style={{ background: "rgba(0,0,0,0.45)" }} onClick={() => setRenameTarget(null)}>
          <div
            className="w-[340px] rounded-xl p-5"
            style={{ background: "var(--surface-card)", border: "1px solid var(--border-strong)", boxShadow: "0 16px 40px rgba(0,0,0,0.4)" }}
            onClick={(e) => e.stopPropagation()}
          >
            <h3 className="m-0 mb-3 text-[14px] font-semibold">重命名会话</h3>
            <Input
              autoFocus
              value={renameValue}
              onChange={(e) => setRenameValue(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && renameValue.trim()) {
                  void renameSession(renameTarget.id, renameValue.trim());
                  setRenameTarget(null);
                }
                if (e.key === "Escape") setRenameTarget(null);
              }}
            />
            <div className="mt-4 flex justify-end gap-2">
              <Button variant="outline" size="sm" onClick={() => setRenameTarget(null)}>取消</Button>
              <Button
                size="sm"
                disabled={!renameValue.trim()}
                onClick={() => {
                  void renameSession(renameTarget.id, renameValue.trim());
                  setRenameTarget(null);
                }}
              >
                保存
              </Button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
