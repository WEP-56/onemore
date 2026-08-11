// 顶部栏:工作区与会话路径、运行状态和窗口级操作。

import { ChevronRight, PanelLeft, Plus, Square, FolderOpen } from "lucide-react";
import { useStore } from "@/app/store";
import { cn } from "@/lib/utils";
import { phaseLabel } from "@/app/util";
import type { LiveActivity } from "@/app/types";

interface MainTopbarProps {
  sidebarCollapsed: boolean;
  onToggleSidebar: () => void;
}

const RUNNING_PHASES = ["running", "retrying", "compacting", "waiting_approval"];

function activityLabel(activity: Exclude<LiveActivity, null>): string {
  switch (activity.kind) {
    case "tool_call_pending":
      return `准备工具：${activity.name}`;
    case "tool":
      return `执行工具：${activity.name}`;
    case "retry":
      return activity.scheduled
        ? `等待重试 ${activity.attempt}/${activity.maxRetries}`
        : `正在重试 ${activity.attempt}/${activity.maxRetries}`;
    case "compaction":
      return activity.trigger === "automatic" ? "自动压缩上下文" : "压缩上下文";
  }
}

export default function MainTopbar({ sidebarCollapsed, onToggleSidebar }: MainTopbarProps) {
  const conn = useStore((s) => s.conn);
  const workspaces = useStore((s) => s.workspaces);
  const activeWorkspace = useStore((s) => s.activeWorkspace);
  const sessions = useStore((s) => s.sessions);
  const snapshot = useStore((s) => s.snapshot);
  const liveActivity = useStore((s) => s.liveActivity);
  const newConversation = useStore((s) => s.newConversation);
  const sendAbort = useStore((s) => s.sendAbort);

  const activeLabel = workspaces.find((w) => w.path === activeWorkspace)?.label ?? activeWorkspace ?? null;
  const activeSession = sessions.find((session) => session.id === snapshot?.session_id);
  const phase = snapshot?.phase ?? "idle";
  const running = RUNNING_PHASES.includes(phase);
  const status = liveActivity ? activityLabel(liveActivity) : running ? phaseLabel(phase) : null;

  return (
    <header className="main-topbar" data-tauri-drag-region>
      <div className="main-topbar-left">
        <button
          type="button"
          className="topbar-icon-button"
          title={sidebarCollapsed ? "展开侧栏" : "折叠侧栏"}
          onClick={onToggleSidebar}
        >
          <PanelLeft size={15} />
        </button>
        {activeLabel ? (
          <div className="topbar-breadcrumbs">
            <FolderOpen size={14} />
            <span>{activeLabel}</span>
            {activeSession && (
              <>
                <ChevronRight size={12} className="topbar-chevron" />
                <strong>{activeSession.title}</strong>
              </>
            )}
            <span
              className={cn(
                "connection-dot",
                conn === "connected" ? "bg-[var(--status-success)]" : conn === "disconnected" ? "bg-[var(--status-error)]" : "bg-[var(--status-warning)]",
              )}
              title={conn}
            />
          </div>
        ) : (
          <span className="topbar-placeholder">OneMore</span>
        )}
      </div>

      <div className="topbar-actions">
        {status && (
          <span className="topbar-phase" title={status}>
            <span />
            {status}
          </span>
        )}
        <button
          type="button"
          className="topbar-icon-button"
          title={running ? "中断" : "新建会话"}
          onClick={() => (running ? void sendAbort() : void newConversation())}
        >
          {running ? <Square size={14} /> : <Plus size={15} />}
        </button>
      </div>
    </header>
  );
}
