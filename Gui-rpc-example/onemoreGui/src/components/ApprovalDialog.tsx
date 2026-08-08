import { useEffect } from "react";
import { useStore } from "@/app/store";

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
    <div
      className="fixed inset-0 z-[100] flex items-center justify-center"
      style={{ background: "rgba(0,0,0,0.5)" }}
    >
      <div
        className="flex w-[460px] max-w-[calc(100vw-48px)] flex-col gap-3 rounded-lg p-6"
        style={{ background: "var(--surface-card)", border: "1px solid var(--border-strong)", boxShadow: "0 8px 24px rgba(0,0,0,0.4)" }}
      >
        <h2 className="m-0 text-base font-semibold">审批请求</h2>
        <div
          className="mono self-start rounded-full px-2.5 py-0.5 text-xs"
          style={{ border: "1px solid var(--status-warning)", color: "var(--status-warning)" }}
        >
          {request.tool}
        </div>
        <p className="m-0 text-sm">{request.summary}</p>
        <p className="m-0 text-[13px] text-[var(--text-faint)]">{request.reason}</p>
        <div className="flex gap-2">
          {request.scopes.map((s) => (
            <span key={s} className="mono rounded-full px-2 py-0.5 text-[11px] text-[var(--text-faint)]" style={{ border: "1px solid var(--border-strong)" }}>
              {s}
            </span>
          ))}
        </div>
        <div className="mt-2 flex justify-end gap-2">
          <button
            type="button"
            className="rounded-md px-3.5 py-1.5 text-[13px] font-semibold text-black disabled:opacity-40"
            style={{ background: "var(--primary)" }}
            disabled={busy}
            onClick={() => void respondApproval("allow_once")}
          >
            Allow Once
          </button>
          <button
            type="button"
            className="rounded-md px-3.5 py-1.5 text-[13px] transition-colors hover:bg-[var(--surface-hover)]"
            style={{ border: "1px solid var(--border-strong)", background: "var(--surface-control)", color: "var(--text-primary)" }}
            disabled={busy}
            onClick={() => void respondApproval("allow_session")}
          >
            Allow Session
          </button>
          <button
            type="button"
            className="rounded-md px-3.5 py-1.5 text-[13px] disabled:opacity-40"
            style={{ border: "1px solid var(--status-error)", color: "var(--status-error)" }}
            disabled={busy}
            onClick={() => void respondApproval("deny")}
          >
            Deny
          </button>
        </div>
        <p className="text-[11px] text-[var(--text-faint)]">Esc = Deny</p>
      </div>
    </div>
  );
}
