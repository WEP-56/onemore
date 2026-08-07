import { useEffect, useState } from "react";
import { useStore } from "../app/store";
import { formatDuration, shortId } from "../app/util";

function useNow(interval = 1000) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), interval);
    return () => clearInterval(id);
  }, [interval]);
  return now;
}

/// 底部状态条：当前 command / elapsed / queue / approval / 最近错误 / revision。
export default function RunMonitor() {
  const phase = useStore((s) => s.snapshot?.phase ?? "idle");
  const queues = useStore((s) => s.snapshot?.queues);
  const run = useStore((s) => s.run);
  const lastTerminal = useStore((s) => s.lastTerminal);
  const lastError = useStore((s) => s.lastError);
  const revision = useStore((s) => s.snapshot?.revision ?? 0);
  const liveApproval = useStore((s) => s.liveApproval);
  const now = useNow();

  const running = phase !== "idle" && phase !== "shutting_down";
  const elapsed = run.startedAt ? now - run.startedAt : 0;
  const steerCount = queues?.steering.length ?? 0;
  const followCount = queues?.follow_up.length ?? 0;

  return (
    <footer className="run-monitor">
      <div className="monitor-block">
        <span className="monitor-label">command</span>
        <span className="mono monitor-value" title={run.commandId ?? lastTerminal?.commandId ?? ""}>
          {run.commandId ? shortId(run.commandId) : lastTerminal?.commandId ? shortId(lastTerminal.commandId) : "—"}
        </span>
        {lastTerminal && (
          <span className={`terminal-chip terminal-${lastTerminal.status}`}>{lastTerminal.status}</span>
        )}
      </div>
      <div className="monitor-block">
        <span className="monitor-label">elapsed</span>
        <span className="mono monitor-value">{running ? formatDuration(elapsed) : "—"}</span>
      </div>
      <div className="monitor-block">
        <span className="monitor-label">queue</span>
        <span className="mono monitor-value">
          steer {steerCount} · follow_up {followCount}
        </span>
      </div>
      <div className="monitor-block">
        <span className="monitor-label">approval</span>
        <span className={`mono monitor-value ${liveApproval ? "warn-text" : ""}`}>
          {liveApproval ? `等待 ${liveApproval.tool}` : "无"}
        </span>
      </div>
      {lastError && (
        <div className="monitor-block monitor-error" title={`${lastError.code}: ${lastError.message}`}>
          <span className="monitor-label">error</span>
          <span className="mono monitor-value">
            {lastError.code}: {lastError.message}
          </span>
        </div>
      )}
      <div className="monitor-spacer" />
      <div className="monitor-block">
        <span className="monitor-label">revision</span>
        <span className="mono monitor-value">{revision}</span>
      </div>
    </footer>
  );
}
