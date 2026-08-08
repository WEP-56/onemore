// 项目:工作区管理(添加/删除/重命名/分组)。

import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useStore } from "@/app/store";
import { Folder, FolderPlus, Pencil, Plus, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { cn } from "@/lib/utils";

export default function ProjectsSection() {
  const workspaces = useStore((s) => s.workspaces);
  const groups = useStore((s) => s.workspaceGroups);
  const addWorkspace = useStore((s) => s.addWorkspace);
  const removeWorkspace = useStore((s) => s.removeWorkspace);
  const renameWorkspace = useStore((s) => s.renameWorkspace);
  const createGroup = useStore((s) => s.createGroup);
  const renameGroup = useStore((s) => s.renameGroup);
  const deleteGroup = useStore((s) => s.deleteGroup);
  const assignGroup = useStore((s) => s.assignGroup);

  const [newGroupName, setNewGroupName] = useState("");
  const [renameTarget, setRenameTarget] = useState<{ id: string; label: string; type: "workspace" | "group" } | null>(null);
  const [renameValue, setRenameValue] = useState("");

  const handleAddWorkspace = async () => {
    const dir = await open({ directory: true, title: "选择工作区目录" });
    if (typeof dir === "string") await addWorkspace(dir);
  };

  const groupedWorkspaces = groups.map((g) => ({
    group: g,
    items: workspaces.filter((w) => w.group_id === g.id),
  }));
  const ungrouped = workspaces.filter((w) => !w.group_id);

  const startRename = (id: string, label: string, type: "workspace" | "group") => {
    setRenameTarget({ id, label, type });
    setRenameValue(label);
  };

  const confirmRename = () => {
    if (!renameTarget || !renameValue.trim()) return;
    if (renameTarget.type === "workspace") void renameWorkspace(renameTarget.id, renameValue.trim());
    else void renameGroup(renameTarget.id, renameValue.trim());
    setRenameTarget(null);
  };

  return (
    <div>
      <div className="settings-section">
        <div className="flex items-center justify-between">
          <h3 className="settings-section-title" style={{ marginBottom: 0 }}>工作区</h3>
          <Button size="sm" onClick={() => void handleAddWorkspace()}>
            <Plus size={13} /> 添加工作区
          </Button>
        </div>
        <p className="settings-section-desc">管理你的项目目录。删除仅从列表移除,不会删除磁盘文件。</p>

        {ungrouped.length > 0 && (
          <div className="settings-workspace-group">
            {groups.length > 0 && <div className="settings-workspace-group-header">未分组</div>}
            {ungrouped.map((w) => (
              <WorkspaceRow
                key={w.path}
                label={w.label}
                path={w.path}
                groups={groups}
                currentGroupId={null}
                onRename={() => startRename(w.path, w.label, "workspace")}
                onRemove={() => void removeWorkspace(w.path)}
                onAssign={(gid) => void assignGroup(w.path, gid)}
              />
            ))}
          </div>
        )}

        {groupedWorkspaces.map(({ group, items }) => (
          <div key={group.id} className="settings-workspace-group">
            <div className="settings-workspace-group-header">
              <FolderPlus size={12} />
              {group.name}
              <span className="text-[10px] opacity-70">{items.length}</span>
              <span className="group-actions">
                <button type="button" className="sidebar-icon-btn" title="重命名分组" onClick={() => startRename(group.id, group.name, "group")}>
                  <Pencil size={11} />
                </button>
                <button type="button" className="sidebar-icon-btn danger" title="删除分组" onClick={() => void deleteGroup(group.id)}>
                  <Trash2 size={11} />
                </button>
              </span>
            </div>
            {items.map((w) => (
              <WorkspaceRow
                key={w.path}
                label={w.label}
                path={w.path}
                groups={groups}
                currentGroupId={group.id}
                onRename={() => startRename(w.path, w.label, "workspace")}
                onRemove={() => void removeWorkspace(w.path)}
                onAssign={(gid) => void assignGroup(w.path, gid)}
              />
            ))}
          </div>
        ))}

        {workspaces.length === 0 && (
          <div className="settings-empty">还没有工作区,点击右上角「添加工作区」。</div>
        )}

        <div className="mt-4 flex items-center gap-2">
          <Input
            className="h-8 w-56"
            placeholder="新建分组名称"
            value={newGroupName}
            onChange={(e) => setNewGroupName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && newGroupName.trim()) {
                void createGroup(newGroupName.trim());
                setNewGroupName("");
              }
            }}
          />
          <Button
            variant="outline"
            size="sm"
            disabled={!newGroupName.trim()}
            onClick={() => {
              void createGroup(newGroupName.trim());
              setNewGroupName("");
            }}
          >
            <FolderPlus size={13} /> 新建分组
          </Button>
        </div>
      </div>

      {renameTarget && (
        <div className="fixed inset-0 z-[120] flex items-center justify-center" style={{ background: "rgba(0,0,0,0.45)" }} onClick={() => setRenameTarget(null)}>
          <div
            className="w-[340px] rounded-xl p-5"
            style={{ background: "var(--surface-card)", border: "1px solid var(--border-strong)", boxShadow: "0 16px 40px rgba(0,0,0,0.4)" }}
            onClick={(e) => e.stopPropagation()}
          >
            <h3 className="m-0 mb-3 text-[14px] font-semibold">重命名{renameTarget.type === "workspace" ? "工作区" : "分组"}</h3>
            <Label className="settings-field-label">名称</Label>
            <Input
              className="mt-1"
              autoFocus
              value={renameValue}
              onChange={(e) => setRenameValue(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") confirmRename();
                if (e.key === "Escape") setRenameTarget(null);
              }}
            />
            <div className="mt-4 flex justify-end gap-2">
              <Button variant="outline" size="sm" onClick={() => setRenameTarget(null)}>取消</Button>
              <Button size="sm" disabled={!renameValue.trim()} onClick={confirmRename}>保存</Button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

function WorkspaceRow({
  label,
  path,
  groups,
  currentGroupId,
  onRename,
  onRemove,
  onAssign,
}: {
  label: string;
  path: string;
  groups: { id: string; name: string }[];
  currentGroupId: string | null;
  onRename: () => void;
  onRemove: () => void;
  onAssign: (groupId: string) => void;
}) {
  return (
    <div className="settings-card" style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 8, padding: "10px 14px" }}>
      <Folder size={15} className="shrink-0 text-[var(--text-faint)]" />
      <div className="min-w-0 flex-1">
        <div className="settings-card-title">{label}</div>
        <div className="settings-card-sub">{path}</div>
      </div>
      <select
        className="h-7 rounded-md border px-1.5 text-[11.5px] outline-none"
        style={{ borderColor: "var(--border-subtle)", background: "var(--surface-control)", color: "var(--text-muted)" }}
        value={currentGroupId ?? ""}
        onChange={(e) => onAssign(e.target.value)}
        title="移动到分组"
      >
        <option value="">未分组</option>
        {groups.map((g) => (
          <option key={g.id} value={g.id}>{g.name}</option>
        ))}
      </select>
      <button type="button" className="sidebar-icon-btn" title="重命名" onClick={onRename}>
        <Pencil size={12} />
      </button>
      <button type="button" className={cn("sidebar-icon-btn danger")} title="移除工作区" onClick={onRemove}>
        <Trash2 size={12} />
      </button>
    </div>
  );
}
