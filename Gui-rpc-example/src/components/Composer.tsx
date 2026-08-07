import { useStore } from "../app/store";
import type { SessionPhase } from "../rpc/protocol";

const RUNNING_PHASES: SessionPhase[] = ["running", "retrying", "compacting", "waiting_approval"];

/// 底部输入区：空闲发 prompt；运行中默认发 steer，可切换 follow_up；可 abort / 快照。
export default function Composer() {
  const conn = useStore((s) => s.conn);
  const phase = useStore((s) => s.snapshot?.phase ?? "idle");
  const draft = useStore((s) => s.draft);
  const setDraft = useStore((s) => s.setDraft);
  const busy = useStore((s) => s.busy);
  const sendPrompt = useStore((s) => s.sendPrompt);
  const sendSteer = useStore((s) => s.sendSteer);
  const sendFollowUp = useStore((s) => s.sendFollowUp);
  const sendAbort = useStore((s) => s.sendAbort);
  const snapshotNow = useStore((s) => s.snapshotNow);

  const connected = conn === "connected";
  const running = RUNNING_PHASES.includes(phase);
  const waitingApproval = phase === "waiting_approval";
  const queueKind = useStore((s) => s.queueKind) as "steer" | "follow_up";

  const setQueueKind = useStore((s) => s.setQueueKind);

  const submit = () => {
    const text = draft.trim();
    if (!text || busy || !connected) return;
    setDraft("");
    if (running) {
      if (queueKind === "follow_up") void sendFollowUp(text);
      else void sendSteer(text);
    } else {
      void sendPrompt(text);
    }
  };

  return (
    <div className="composer">
      {running && (
        <div className="composer-mode">
          <div className="composer-seg" role="group" aria-label="运行中输入方式">
            <button
              type="button"
              className={`seg-btn ${queueKind === "steer" ? "active" : ""}`}
              onClick={() => setQueueKind("steer")}
              title="在当前完整工具批后注入方向修正"
            >
              steer
            </button>
            <button
              type="button"
              className={`seg-btn ${queueKind === "follow_up" ? "active" : ""}`}
              onClick={() => setQueueKind("follow_up")}
              title="在当前任务将停止时追加工作"
            >
              follow_up
            </button>
          </div>
          <span className="hint">
            运行中：Enter 发送 {queueKind}
            {waitingApproval && "（等待审批，仍可排队输入）"}
          </span>
        </div>
      )}
      <div className="composer-row">
        <textarea
          className="input composer-input"
          rows={2}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              submit();
            }
          }}
          placeholder={
            !connected
              ? "请先连接 Onemore"
              : running
                ? "运行中：输入 steer 或 follow-up…"
                : "输入 prompt 开始对话…"
          }
          spellCheck={false}
        />
        <div className="composer-actions">
          <button
            type="button"
            className="btn btn-quiet"
            disabled={!connected}
            title="发送 get_snapshot 做一次权威校正"
            onClick={() => void snapshotNow()}
          >
            快照
          </button>
          {running ? (
            <button
              type="button"
              className="btn btn-danger"
              disabled={!connected}
              title="请求取消当前任务（等待每个 accepted command 的 terminal）"
              onClick={() => void sendAbort()}
            >
              中止
            </button>
          ) : null}
          <button
            type="button"
            className="btn btn-accent"
            disabled={!connected || !draft.trim() || busy}
            onClick={submit}
          >
            {running ? `发送 ${queueKind}` : "发送"}
          </button>
        </div>
      </div>
    </div>
  );
}
