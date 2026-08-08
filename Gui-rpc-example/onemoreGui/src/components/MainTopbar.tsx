// 顶部栏:工作区信息 + 模型选择 + 用量 + 操作。视觉参照 desktop-cc-gui MainTopbar。

import { PanelLeft, Settings, Plus, Square, FolderOpen } from "lucide-react";
import { useStore } from "@/app/store";
import { cn } from "@/lib/utils";
import { formatTokens, phaseLabel } from "@/app/util";
import { ModelSelectMenu } from "@/components/ModelSelectMenu";

interface MainTopbarProps {
  sidebarCollapsed: boolean;
  onToggleSidebar: () => void;
  onOpenSettings: () => void;
}

const RUNNING_PHASES = ["running", "retrying", "compacting", "waiting_approval"];

export default function MainTopbar({ sidebarCollapsed, onToggleSidebar, onOpenSettings }: MainTopbarProps) {
  const conn = useStore((s) => s.conn);
  const workspaces = useStore((s) => s.workspaces);
  const activeWorkspace = useStore((s) => s.activeWorkspace);
  const snapshot = useStore((s) => s.snapshot);
  const clearConversation = useStore((s) => s.clearConversation);
  const sendAbort = useStore((s) => s.sendAbort);

  const activeLabel = workspaces.find((w) => w.path === activeWorkspace)?.label ?? activeWorkspace ?? null;
  const phase = snapshot?.phase ?? "idle";
  const usage = snapshot?.usage;
  const running = RUNNING_PHASES.includes(phase);

  return (
    <header className="main-topbar">
      <div className="main-topbar-left">
        <button
          type="button"
          className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-[var(--text-muted)] transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--text-strong)]"
          title={sidebarCollapsed ? "展开侧栏" : "折叠侧栏"}
          onClick={onToggleSidebar}
        >
          <PanelLeft size={15} />
        </button>
        {activeLabel ? (
          <div className="flex min-w-0 items-center gap-2">
            <FolderOpen size={15} className="shrink-0 text-[var(--status-success)]" />
            <span className="truncate text-[13px] font-semibold text-[var(--text-strong)]">{activeLabel}</span>
            <span
              className={cn(
                "h-1.5 w-1.5 shrink-0 rounded-full",
                conn === "connected" ? "bg-[var(--status-success)]" : conn === "disconnected" ? "bg-[var(--status-error)]" : "bg-[var(--status-warning)]",
              )}
              style={conn === "connected" ? { boxShadow: "0 0 6px var(--status-success)" } : undefined}
              title={conn}
            />
          </div>
        ) : (
          <span className="text-[13px] text-[var(--text-faint)]">未选择工作区</span>
        )}
      </div>

      <div className="actions">
        <ModelSelectMenu />
        {usage && (usage.input_tokens > 0 || usage.output_tokens > 0) && (
          <span className="mono hidden items-center gap-1 rounded-md px-2 py-1 text-[11px] text-[var(--text-faint)] xl:flex" title="Token 用量">
            ↑{formatTokens(usage.input_tokens)} ↓{formatTokens(usage.output_tokens)}
          </span>
        )}
        {running && (
          <span className="flex items-center gap-1.5 rounded-md px-2 py-1 text-[11px] text-[var(--status-warning)]">
            <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-[var(--status-warning)]" />
            {phaseLabel(phase)}
          </span>
        )}
        <button
          type="button"
          className="flex h-7 w-7 items-center justify-center rounded-md text-[var(--text-muted)] transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--text-strong)]"
          title={running ? "中断" : "新建会话"}
          onClick={() => (running ? void sendAbort() : void clearConversation())}
        >
          {running ? <Square size={14} /> : <Plus size={15} />}
        </button>
        <button
          type="button"
          className="flex h-7 w-7 items-center justify-center rounded-md text-[var(--text-muted)] transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--text-strong)]"
          title="设置"
          onClick={onOpenSettings}
        >
          <Settings size={15} />
        </button>
      </div>
    </header>
  );
}
