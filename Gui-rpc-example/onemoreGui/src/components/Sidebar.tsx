import { useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useStore } from "@/app/store";
import { normalizeWorkspace, relativeTime, workspaceKey } from "@/app/util";
import {
  Plus,
  Search,
  MessageSquare,
  Folder,
  Trash2,
  ChevronDown,
  ChevronRight,
  Pin,
  PinOff,
  Pencil,
  FolderPlus,
  X,
  Check,
  GitBranch,
  MoreHorizontal,
  Home,
  Settings,
  RefreshCw,
} from "lucide-react";
import { cn } from "@/lib/utils";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import BrandMark from "@/components/BrandMark";

export default function Sidebar() {
  const workspaces = useStore((s) => s.workspaces);
  const workspaceGroups = useStore((s) => s.workspaceGroups);
  const activeWorkspace = useStore((s) => s.activeWorkspace);
  const sessions = useStore((s) => s.sessions);
  const searchQuery = useStore((s) => s.searchQuery);
  const setSearchQuery = useStore((s) => s.setSearchQuery);
  const addWorkspace = useStore((s) => s.addWorkspace);
  const selectWorkspace = useStore((s) => s.selectWorkspace);
  const connect = useStore((s) => s.connect);
  const loadSessions = useStore((s) => s.loadSessions);
  const loadSession = useStore((s) => s.loadSession);
  const newConversation = useStore((s) => s.newConversation);
  const snapshot = useStore((s) => s.snapshot);
  const setSettingsOpen = useStore((s) => s.setSettingsOpen);
  const conn = useStore((s) => s.conn);

  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
  const [renameTarget, setRenameTarget] = useState<{ type: "workspace" | "session"; id: string; title: string } | null>(null);
  const [groupDialog, setGroupDialog] = useState<{ mode: "create" } | null>(null);

  const handleAddWorkspace = async () => {
    const dir = await open({ directory: true, title: "选择工作区目录" });
    if (typeof dir === "string") await addWorkspace(dir);
  };

  const handleSelectWorkspace = async (path: string) => {
    await selectWorkspace(path);
    await connect(path);
    await loadSessions();
  };

  const handleExpand = (path: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  // 会话按工作区分组 + 过滤
  const { sessionsByWorkspace, searchActive } = useMemo(() => {
    const q = searchQuery.trim().toLowerCase();
    const byWs = new Map<string, typeof sessions>();
    for (const s of sessions) {
      const norm = normalizeWorkspace(s.workspace);
      if (q) {
        if (!s.title.toLowerCase().includes(q) && !norm.toLowerCase().includes(q)) continue;
      }
      const key = workspaceKey(s.workspace);
      const list = byWs.get(key) ?? [];
      list.push(s);
      byWs.set(key, list);
    }
    for (const list of byWs.values()) list.sort((a, b) => b.updated_at - a.updated_at);
    return { sessionsByWorkspace: byWs, searchActive: Boolean(q) };
  }, [sessions, searchQuery]);

  // 工作区按组归类
  const grouped = useMemo(() => {
    const groups = workspaceGroups.map((g) => ({
      group: g,
      workspaces: workspaces.filter((w) => w.group_id === g.id),
    }));
    const ungrouped = workspaces.filter((w) => !w.group_id);
    return { groups, ungrouped };
  }, [workspaces, workspaceGroups]);

  const handleSelectSession = (id: string) => {
    void loadSession(id);
  };

  const isExpanded = (path: string) => expanded.has(path) || searchActive || path === activeWorkspace;

  return (
    <aside className="sidebar">
      <div className="sidebar-brand" data-tauri-drag-region>
        <BrandMark />
        <span>OneMore</span>
      </div>
      <nav className="sidebar-primary-nav" aria-label="主导航">
        <button type="button" className="sidebar-primary-item is-active">
          <Home size={15} />
          <span>首页</span>
        </button>
        <button type="button" className="sidebar-primary-item" disabled={conn !== "connected"} onClick={() => void newConversation()}>
          <Plus size={15} />
          <span>新建会话</span>
          <kbd>Ctrl N</kbd>
        </button>
      </nav>
      <div className="sidebar-search-box">
        <Search size={13} className="shrink-0" />
        <input
          placeholder="搜索会话…"
          value={searchQuery}
          onChange={(e) => setSearchQuery(e.target.value)}
          spellCheck={false}
        />
        {searchQuery && (
          <button type="button" className="sidebar-icon-btn" onClick={() => setSearchQuery("")}>
            <X size={12} />
          </button>
        )}
      </div>

      <div className="sidebar-body">
        <div className="sidebar-body-layout">
          <div className="sidebar-content-column">
            {/* 工作区 */}
            <div className="sidebar-section-header">
              <span className="sidebar-section-title">工作区</span>
              <span className="flex items-center gap-1">
                <button
                  type="button"
                  className="sidebar-title-add"
                  title="新建分组"
                  onClick={() => setGroupDialog({ mode: "create" })}
                >
                  <FolderPlus size={12} />
                </button>
                <button
                  type="button"
                  className="sidebar-title-add"
                  title="添加工作区"
                  onClick={() => void handleAddWorkspace()}
                >
                  <Plus size={13} />
                </button>
              </span>
            </div>
            <div className="workspace-list">
              {grouped.groups.map(({ group, workspaces: groupWorkspaces }) => (
                <div key={group.id} className="workspace-group">
                  <WorkspaceGroupHeader
                    name={group.name}
                    count={groupWorkspaces.length}
                    onRename={() => setRenameTarget({ type: "workspace", id: group.id, title: group.name })}
                    onDelete={() => void useStore.getState().deleteGroup(group.id)}
                  />
                  {groupWorkspaces.map((w) => (
                    <WorkspaceRow
                      key={w.path}

                      label={w.label}
                      active={activeWorkspace === w.path}
                      expanded={isExpanded(w.path)}
                      hasSessions={(sessionsByWorkspace.get(workspaceKey(w.path))?.length ?? 0) > 0}
                      onSelect={() => void handleSelectWorkspace(w.path)}
                      onToggle={() => handleExpand(w.path)}
                      onRename={() => setRenameTarget({ type: "workspace", id: w.path, title: w.label })}
                      onRemove={() => void useStore.getState().removeWorkspace(w.path)}
                      groupOptions={workspaceGroups}
                      currentGroupId={w.group_id ?? null}
                      onAssignGroup={(gid) => void useStore.getState().assignGroup(w.path, gid)}
                      sessions={sessionsByWorkspace.get(workspaceKey(w.path)) ?? []}
                      activeSessionId={snapshot?.session_id ?? null}
                      onSelectSession={handleSelectSession}
                      onRenameSession={(id, title) => setRenameTarget({ type: "session", id, title })}
                      searchActive={searchActive}
                    />
                  ))}
                </div>
              ))}
              {grouped.ungrouped.length > 0 && (
                <div className="workspace-group">
                  {grouped.groups.length > 0 && (
                    <div className="workspace-group-header" style={{ cursor: "default" }}>
                      <span className="workspace-group-title">未分组</span>
                    </div>
                  )}
                  {grouped.ungrouped.map((w) => (
                    <WorkspaceRow
                      key={w.path}

                      label={w.label}
                      active={activeWorkspace === w.path}
                      expanded={isExpanded(w.path)}
                      hasSessions={(sessionsByWorkspace.get(workspaceKey(w.path))?.length ?? 0) > 0}
                      onSelect={() => void handleSelectWorkspace(w.path)}
                      onToggle={() => handleExpand(w.path)}
                      onRename={() => setRenameTarget({ type: "workspace", id: w.path, title: w.label })}
                      onRemove={() => void useStore.getState().removeWorkspace(w.path)}
                      groupOptions={workspaceGroups}
                      currentGroupId={w.group_id ?? null}
                      onAssignGroup={(gid) => void useStore.getState().assignGroup(w.path, gid)}
                      sessions={sessionsByWorkspace.get(workspaceKey(w.path)) ?? []}
                      activeSessionId={snapshot?.session_id ?? null}
                      onSelectSession={handleSelectSession}
                      onRenameSession={(id, title) => setRenameTarget({ type: "session", id, title })}
                      searchActive={searchActive}
                    />
                  ))}
                </div>
              )}
              {workspaces.length === 0 && (
                <div className="sidebar-empty">还没有工作区。点击上方 + 添加项目目录。</div>
              )}
            </div>
          </div>
        </div>
      </div>

      <div className="sidebar-footer">
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button type="button" className="sidebar-settings-button" title="快捷菜单">
              <Settings size={15} />
              <span>菜单</span>
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent side="top" align="start" sideOffset={7} className="sidebar-corner-menu w-52">
            <DropdownMenuLabel>OneMore</DropdownMenuLabel>
            <DropdownMenuItem disabled={conn !== "connected"} onSelect={() => void newConversation()}>
              <Plus size={14} /> 新建会话
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => void handleAddWorkspace()}>
              <FolderPlus size={14} /> 添加项目
            </DropdownMenuItem>
            <DropdownMenuItem disabled={conn !== "connected"} onSelect={() => void loadSessions()}>
              <RefreshCw size={14} /> 刷新会话
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => setSettingsOpen(true)}>
              <Settings size={14} /> 设置
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <span className="sidebar-version">v0.1.0</span>
      </div>

      <RenameDialog target={renameTarget} onClose={() => setRenameTarget(null)} />
      <CreateGroupDialog
        open={groupDialog !== null}
        onClose={() => setGroupDialog(null)}
        onConfirm={(name) => void useStore.getState().createGroup(name)}
      />
    </aside>
  );
}

/* ── 分组头 ── */
function WorkspaceGroupHeader({
  name,
  count,
  onRename,
  onDelete,
}: {
  name: string;
  count: number;
  onRename: () => void;
  onDelete: () => void;
}) {
  return (
    <div className="workspace-group-header">
      <span className="workspace-group-title">
        {name}
        <span className="text-[10px] text-[var(--text-dim)]">{count}</span>
      </span>
      <span className="workspace-group-actions">
        <button type="button" className="sidebar-icon-btn" title="重命名分组" onClick={onRename}>
          <Pencil size={11} />
        </button>
        <button type="button" className="sidebar-icon-btn danger" title="删除分组" onClick={onDelete}>
          <Trash2 size={11} />
        </button>
      </span>
    </div>
  );
}

/* ── 工作区行(含其会话列表) ── */
function WorkspaceRow({
  label,
  active,
  expanded,
  hasSessions,
  onSelect,
  onToggle,
  onRename,
  onRemove,
  groupOptions,
  currentGroupId,
  onAssignGroup,
  sessions,
  activeSessionId,
  onSelectSession,
  onRenameSession,
  searchActive,
}: {
  label: string;
  active: boolean;
  expanded: boolean;
  hasSessions: boolean;
  onSelect: () => void;
  onToggle: () => void;
  onRename: () => void;
  onRemove: () => void;
  groupOptions: { id: string; name: string }[];
  currentGroupId: string | null;
  onAssignGroup: (groupId: string) => void;
  sessions: import("@/app/types").SessionEntry[];
  activeSessionId: string | null;
  onSelectSession: (id: string) => void;
  onRenameSession: (id: string, title: string) => void;
  searchActive: boolean;
}) {
  const git = useStore((s) => s.gitStatus);
  const showBranch = active && git?.is_repo && git.branch;

  return (
    <div className="workspace-card">
      <div className={cn("workspace-row", active && "active")}>
        <div className="workspace-header-content">
          <button
            type="button"
            className="workspace-tree-toggle"
            onClick={(e) => {
              e.stopPropagation();
              if (hasSessions || searchActive) onToggle();
            }}
            aria-label={expanded ? "收起会话" : "展开会话"}
          >
            {expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
          </button>
          <div className="flex min-w-0 flex-1 items-center gap-2" onClick={onSelect} role="button" tabIndex={0}>
            <Folder size={15} className="default-workspace-folder-icon" />
            <span className="workspace-name-text">{label}</span>
            {showBranch && (
              <span className="workspace-branch-badge" title={git.branch}>
                <GitBranch size={10} />
                {git.branch}
              </span>
            )}
          </div>
          <span className="workspace-actions">
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button type="button" className="sidebar-icon-btn" title="更多">
                  <MoreHorizontal size={13} />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start" className="w-44">
                <DropdownMenuLabel>{label}</DropdownMenuLabel>
                <DropdownMenuSeparator />
                <DropdownMenuItem onSelect={onRename}>
                  <Pencil size={13} /> 重命名
                </DropdownMenuItem>
                <DropdownMenuItem
                  onSelect={() => {
                    if (currentGroupId) onAssignGroup("");
                  }}
                  disabled={!currentGroupId}
                >
                  <Folder size={13} /> 移出分组
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  className="text-[var(--status-error)]"
                  onSelect={onRemove}
                >
                  <Trash2 size={13} /> 移除工作区
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button type="button" className="sidebar-icon-btn" title="移动到分组">
                  <FolderPlus size={13} />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start" className="w-44">
                <DropdownMenuLabel>移动到分组</DropdownMenuLabel>
                <DropdownMenuSeparator />
                <DropdownMenuItem onSelect={() => onAssignGroup("")}>
                  <Check size={13} className={cn(currentGroupId === null && "opacity-100", currentGroupId !== null && "opacity-0")} />
                  未分组
                </DropdownMenuItem>
                {groupOptions.map((g) => (
                  <DropdownMenuItem key={g.id} onSelect={() => onAssignGroup(g.id)}>
                    <Check size={13} className={cn(currentGroupId === g.id ? "opacity-100" : "opacity-0")} />
                    {g.name}
                  </DropdownMenuItem>
                ))}
              </DropdownMenuContent>
            </DropdownMenu>
          </span>
        </div>
      </div>
      <div className={cn("workspace-children", !expanded && "is-collapsed")}>
        <div className="workspace-children-inner">
          {sessions.length === 0 && expanded && !searchActive && (
            <div className="sidebar-empty" style={{ textAlign: "left", padding: "2px 10px" }}>
              暂无会话
            </div>
          )}
          {sessions.map((s) => (
            <SessionRow
              key={s.id}
              session={s}
              active={activeSessionId === s.id}
              onSelect={() => onSelectSession(s.id)}
              onRename={() => onRenameSession(s.id, s.title)}
            />
          ))}
        </div>
      </div>
    </div>
  );
}

/* ── 会话行 ── */
function SessionRow({
  session,
  active,
  onSelect,
  onRename,
}: {
  session: import("@/app/types").SessionEntry;
  active: boolean;
  onSelect: () => void;
  onRename: () => void;
}) {
  const isPinned = useStore((s) => s.isSessionPinned(session.id));
  const togglePin = useStore((s) => s.togglePinSession);
  const deleteSession = useStore((s) => s.deleteSession);
  const [confirmDelete, setConfirmDelete] = useState(false);

  return (
    <button
      type="button"
      className={cn("thread-row", active && "active")}
      onClick={onSelect}
      title={session.title}
    >
      {isPinned ? (
        <Pin size={13} className="thread-pin-icon" />
      ) : (
        <MessageSquare size={13} className="thread-icon" />
      )}
      <span className="thread-name">{session.title || "（无标题）"}</span>
      <span className="thread-meta">{relativeTime(session.updated_at)}</span>
      <span className="thread-actions" onClick={(e) => e.stopPropagation()}>
        <button
          type="button"
          className="sidebar-icon-btn"
          title={isPinned ? "取消置顶" : "置顶"}
          onClick={() => togglePin(session.id)}
        >
          {isPinned ? <PinOff size={11} /> : <Pin size={11} />}
        </button>
        <button
          type="button"
          className="sidebar-icon-btn"
          title="重命名"
          onClick={onRename}
        >
          <Pencil size={11} />
        </button>
        {confirmDelete ? (
          <span className="flex items-center gap-0.5 rounded bg-[var(--surface-control)] px-1">
            <button
              type="button"
              className="sidebar-icon-btn"
              title="确认删除"
              onClick={() => {
                setConfirmDelete(false);
                void deleteSession(session.id);
              }}
            >
              <Check size={11} className="text-[var(--status-success)]" />
            </button>
            <button type="button" className="sidebar-icon-btn" title="取消" onClick={() => setConfirmDelete(false)}>
              <X size={11} />
            </button>
          </span>
        ) : (
          <button
            type="button"
            className="sidebar-icon-btn danger"
            title="删除"
            onClick={() => setConfirmDelete(true)}
          >
            <Trash2 size={11} />
          </button>
        )}
      </span>
    </button>
  );
}

/* ── 重命名对话框 ── */
function RenameDialog({
  target,
  onClose,
}: {
  target: { type: "workspace" | "session"; id: string; title: string } | null;
  onClose: () => void;
}) {
  const [value, setValue] = useState("");
  const open = target !== null;

  useEffect(() => {
    if (target) setValue(target.title);
  }, [target]);
  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) onClose();
      }}
    >
      <DialogContent className="w-[340px]">
        <DialogHeader>
          <DialogTitle>重命名{target?.type === "workspace" ? "工作区" : "会话"}</DialogTitle>
        </DialogHeader>
        {target && (
          <Input
            autoFocus
            defaultValue={target.title}
            onChange={(e) => setValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && value.trim()) {
                if (target.type === "workspace") {
                  void useStore.getState().renameWorkspace(target.id, value.trim());
                } else {
                  void useStore.getState().renameSession(target.id, value.trim());
                }
                onClose();
              }
            }}
          />
        )}
        <DialogFooter>
          <Button variant="outline" size="sm" onClick={onClose}>
            取消
          </Button>
          <Button
            size="sm"
            disabled={!value.trim()}
            onClick={() => {
              if (!target) return;
              if (target.type === "workspace") {
                void useStore.getState().renameWorkspace(target.id, value.trim());
              } else {
                void useStore.getState().renameSession(target.id, value.trim());
              }
              onClose();
            }}
          >
            保存
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/* ── 新建分组对话框 ── */
function CreateGroupDialog({
  open,
  onClose,
  onConfirm,
}: {
  open: boolean;
  onClose: () => void;
  onConfirm: (name: string) => void;
}) {
  const [value, setValue] = useState("");
  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) {
          setValue("");
          onClose();
        }
      }}
    >
      <DialogContent className="w-[340px]">
        <DialogHeader>
          <DialogTitle>新建分组</DialogTitle>
        </DialogHeader>
        <Input
          autoFocus
          placeholder="分组名称"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && value.trim()) {
              onConfirm(value.trim());
              setValue("");
              onClose();
            }
          }}
        />
        <DialogFooter>
          <Button variant="outline" size="sm" onClick={onClose}>
            取消
          </Button>
          <Button
            size="sm"
            disabled={!value.trim()}
            onClick={() => {
              onConfirm(value.trim());
              setValue("");
              onClose();
            }}
          >
            创建
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
