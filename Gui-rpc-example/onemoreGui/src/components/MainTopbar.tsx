// 顶部栏:工作区与会话路径、运行状态和窗口级操作。

import { ChevronRight, PanelLeft, Plus, Square, FolderOpen } from "lucide-react";
import { useStore } from "@/app/store";
import { cn } from "@/lib/utils";
import { phaseLabel } from "@/app/util";

interface MainTopbarProps {
  sidebarCollapsed: boolean;
  onToggleSidebar: () => void;
}

const RUNNING_PHASES = ["running", "retrying", "compacting", "waiting_approval"];

export default function MainTopbar({ sidebarCollapsed, onToggleSidebar }: MainTopbarProps) {
  const conn = useStore((s) => s.conn);
  const workspaces = useStore((s) => s.workspaces);
  const activeWorkspace = useStore((s) => s.activeWorkspace);
  const sessions = useStore((s) => s.sessions);
  const snapshot = useStore((s) => s.snapshot);
  const newConversation = useStore((s) => s.newConversation);
  const sendAbort = useStore((s) => s.sendAbort);

  const activeLabel = workspaces.find((w) => w.path === activeWorkspace)?.label ?? activeWorkspace ?? null;
  const activeSession = sessions.find((session) => session.id === snapshot?.session_id);
  const phase = snapshot?.phase ?? "idle";
  const running = RUNNING_PHASES.includes(phase);

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
        {running && (
          <span className="topbar-phase">
            <span />
            {phaseLabel(phase)}
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
