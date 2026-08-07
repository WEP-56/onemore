import { useStore } from "../app/store";
import { formatTokens, phaseLabel } from "../app/util";
import CopyId from "./CopyId";

const CONN_LABEL: Record<string, string> = {
  disconnected: "未连接",
  spawning: "启动中",
  handshaking: "握手",
  connected: "已连接",
  shutting_down: "关闭中",
};

export default function TopBar() {
  const conn = useStore((s) => s.conn);
  const server = useStore((s) => s.server);
  const snapshot = useStore((s) => s.snapshot);
  const disconnect = useStore((s) => s.disconnect);

  const phase = snapshot?.phase ?? "idle";
  const usage = snapshot?.usage;

  return (
    <header className="topbar">
      <div className="brand">
        <span className="brand-dot" aria-hidden />
        <span className="brand-name">Onemore RPC</span>
      </div>
      {server && (
        <>
          <span className="topbar-sep" />
          <span className="topbar-item label">server</span>
          <CopyId value={server.server_id} len={8} />
        </>
      )}
      {snapshot && (
        <>
          <span className="topbar-sep" />
          <span className="topbar-item workspace" title={snapshot.workspace}>
            {snapshot.workspace}
          </span>
          <span className="topbar-sep" />
          <span className="topbar-item model" title={snapshot.model.label}>
            {snapshot.model.label}
          </span>
          <span className="topbar-sep" />
          <span className={`phase-chip phase-${phase}`}>
            <span className="phase-dot" aria-hidden />
            {phaseLabel(phase)}
          </span>
          {usage && (
            <>
              <span className="topbar-sep" />
              <span className="topbar-item usage mono">
                in {formatTokens(usage.input_tokens)} · out {formatTokens(usage.output_tokens)}
                {usage.cache_read_tokens ? ` · cache ${formatTokens(usage.cache_read_tokens)}` : ""}
              </span>
            </>
          )}
          <span className="topbar-sep" />
          <span className="topbar-item mono">rev {snapshot.revision}</span>
        </>
      )}
      <div className="topbar-spacer" />
      <span className={`conn-chip conn-${conn}`}>{CONN_LABEL[conn]}</span>
      {conn !== "disconnected" && (
        <button
          type="button"
          className="btn btn-quiet"
          disabled={conn === "shutting_down"}
          onClick={() => void disconnect()}
        >
          断开
        </button>
      )}
    </header>
  );
}
