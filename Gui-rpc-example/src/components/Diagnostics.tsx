import { useEffect, useRef } from "react";
import { useStore } from "../app/store";

/// 诊断 tab：stderr 与 transport/protocol 错误分栏，默认隐藏完整日志。
export default function Diagnostics() {
  const stderrLines = useStore((s) => s.stderrLines);
  const transportIssues = useStore((s) => s.transportIssues);
  const metrics = useStore((s) => s.metrics);
  const refreshDiagnostics = useStore((s) => s.refreshDiagnostics);
  const clearLogs = useStore((s) => s.clearLogs);
  const logRef = useRef<HTMLPreElement>(null);

  useEffect(() => {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [stderrLines.length]);

  return (
    <div className="diagnostics">
      <div className="diag-toolbar">
        <button type="button" className="btn btn-quiet" onClick={() => void refreshDiagnostics()}>
          刷新 stderr
        </button>
        <button type="button" className="btn btn-quiet" onClick={clearLogs}>
          清空本地日志
        </button>
        <span className="hint">
          stdout 仅按 JSONL 协议解析；stderr 与 transport 错误使用 GUI 自己的 DTO，不伪装成协议事件
        </span>
      </div>
      <div className="diag-columns">
        <section className="diag-col">
          <h3>stderr 诊断（最近 {stderrLines.length} 行，最多 400）</h3>
          <pre ref={logRef} className="diag-log mono">
            {stderrLines.length ? stderrLines.join("\n") : "（无输出）"}
          </pre>
        </section>
        <section className="diag-col">
          <h3>transport / 协议错误</h3>
          {transportIssues.length === 0 ? (
            <p className="muted">（无）</p>
          ) : (
            <ul className="issue-list">
              {transportIssues.map((it, i) => (
                <li key={i} className="issue-item">
                  <span className="mono issue-code">{it.code}</span>
                  <span className="issue-msg">{it.message}</span>
                  <span className="muted mono issue-time">{new Date(it.at).toLocaleTimeString()}</span>
                </li>
              ))}
            </ul>
          )}
          <h3>事件统计</h3>
          <dl className="kv">
            <dt>events / snapshot / progress</dt>
            <dd>{metrics.events} / {metrics.snapshots} / {metrics.progresses}</dd>
            <dt>command_finished / settled</dt>
            <dd>{metrics.commandFinished} / {metrics.settled}</dd>
            <dt>tool started / finished</dt>
            <dd>{metrics.toolsStarted} / {metrics.toolsFinished}</dd>
            <dt>delta 字符</dt>
            <dd>{metrics.assistantDeltaChars}</dd>
          </dl>
        </section>
      </div>
    </div>
  );
}
