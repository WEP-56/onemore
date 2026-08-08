// 审批:右下角浮动卡片,不阻塞界面。视觉参照 cc-gui ApprovalToasts。

import { useEffect } from "react";
import { useStore } from "@/app/store";
import { ShieldAlert, X } from "lucide-react";

export default function ApprovalDialog() {
  const request = useStore((s) => s.liveApproval ?? s.snapshot?.pending_approval ?? null);
  const respondApproval = useStore((s) => s.respondApproval);
  const busy = useStore((s) => s.busy);

  useEffect(() => {
    if (!request) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) void respondApproval("deny");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [request, busy, respondApproval]);

  if (!request) return null;

  const canSession = request.scopes.includes("session");

  return (
    <div className="approval-toast-host">
      <div className="approval-toast">
        <div className="approval-toast-header">
          <div className="approval-toast-header-main">
            <div className="approval-toast-icon-wrap">
              <ShieldAlert size={18} />
            </div>
            <div className="approval-toast-header-copy">
              <div className="approval-toast-title">等待审批</div>
              <div className="approval-toast-tool">{request.tool}</div>
            </div>
          </div>
          <button
            type="button"
            className="flex h-6 w-6 items-center justify-center rounded text-[var(--text-faint)] transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--text-strong)]"
            title="拒绝 (Esc)"
            onClick={() => void respondApproval("deny")}
          >
            <X size={13} />
          </button>
        </div>
        <div className="approval-toast-summary">{request.summary}</div>
        {request.reason && <div className="approval-toast-reason">{request.reason}</div>}
        <div className="approval-toast-actions">
          {canSession && (
            <button
              type="button"
              className="approval-btn approval-btn-secondary"
              disabled={busy}
              onClick={() => void respondApproval("allow_session")}
            >
              本次会话允许
            </button>
          )}
          <button
            type="button"
            className="approval-btn approval-btn-primary"
            disabled={busy}
            onClick={() => void respondApproval("allow_once")}
          >
            允许一次
          </button>
          <button
            type="button"
            className="approval-btn approval-btn-deny"
            disabled={busy}
            onClick={() => void respondApproval("deny")}
          >
            拒绝
          </button>
        </div>
        <div className="approval-toast-hint">Esc = 拒绝</div>
      </div>
    </div>
  );
}
