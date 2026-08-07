import { useEffect } from "react";
import { useStore } from "../app/store";

/// 阻塞式审批 modal：展示 tool / summary / reason / scopes，提供 Once / Session / Deny。
/// 窗口关闭或断连时 fail closed（后端按 deny 收尾，前端回到 disconnected）。
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

  return (
    <div className="modal-overlay" role="presentation">
      <div className="modal approval-modal" role="dialog" aria-modal="true" aria-label="审批请求">
        <h2 className="modal-title">审批请求</h2>
        <div className="approval-tool mono">{request.tool}</div>
        <p className="approval-summary">{request.summary}</p>
        <p className="approval-reason muted">{request.reason}</p>
        <div className="approval-scopes">
          {request.scopes.map((s) => (
            <span key={s} className="scope-chip mono">
              {s}
            </span>
          ))}
        </div>
        <div className="modal-actions">
          <button
            type="button"
            className="btn btn-accent"
            disabled={busy}
            onClick={() => void respondApproval("allow_once")}
          >
            Allow Once
          </button>
          <button
            type="button"
            className="btn"
            disabled={busy}
            onClick={() => void respondApproval("allow_session")}
          >
            Allow Session
          </button>
          <button
            type="button"
            className="btn btn-danger"
            disabled={busy}
            onClick={() => void respondApproval("deny")}
          >
            Deny
          </button>
        </div>
        <p className="muted small">Esc = Deny；窗口关闭或断连时按 deny 收尾（fail closed）。</p>
      </div>
    </div>
  );
}
