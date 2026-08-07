import { open } from "@tauri-apps/plugin-dialog";
import { useStore } from "../app/store";

/// 断连状态的首屏：选择 workspace / config / onemore 可执行文件后连接。
export default function ConnectPanel() {
  const connectOptions = useStore((s) => s.connectOptions);
  const setConnectOptions = useStore((s) => s.setConnectOptions);
  const connect = useStore((s) => s.connect);
  const conn = useStore((s) => s.conn);
  const lastError = useStore((s) => s.lastError);
  const busy = conn === "spawning" || conn === "handshaking";

  const pickWorkspace = async () => {
    const dir = await open({ directory: true, title: "选择工作区" });
    if (typeof dir === "string") setConnectOptions({ workspace: dir });
  };
  const pickExecutable = async () => {
    const f = await open({
      multiple: false,
      title: "选择 onemore 可执行文件",
      filters: [{ name: "onemore", extensions: ["exe", "cmd", "bat"] }],
    });
    if (typeof f === "string") setConnectOptions({ executable: f });
  };
  const pickConfig = async () => {
    const f = await open({
      multiple: false,
      title: "选择 config 文件",
      filters: [
        { name: "config", extensions: ["toml", "conf"] },
        { name: "all", extensions: ["*"] },
      ],
    });
    if (typeof f === "string") setConnectOptions({ config: f });
  };

  return (
    <main className="connect-wrap">
      <div className="connect-panel">
        <div className="connect-head">
          <span className="brand-dot" aria-hidden />
          <h1>连接 Onemore</h1>
          <p className="muted">选择工作区并启动 <code className="mono">onemore --rpc</code> 子进程。</p>
        </div>

        <label className="field">
          <span className="field-label">工作区（必选）</span>
          <div className="field-row">
            <input
              className="input mono"
              value={connectOptions.workspace}
              onChange={(e) => setConnectOptions({ workspace: e.target.value })}
              placeholder="E:\work\project"
              spellCheck={false}
            />
            <button type="button" className="btn btn-quiet" onClick={() => void pickWorkspace()}>
              浏览…
            </button>
          </div>
        </label>

        <label className="field">
          <span className="field-label">Onemore 可执行文件（默认从 PATH 查找）</span>
          <div className="field-row">
            <input
              className="input mono"
              value={connectOptions.executable}
              onChange={(e) => setConnectOptions({ executable: e.target.value })}
              placeholder="onemore"
              spellCheck={false}
            />
            <button type="button" className="btn btn-quiet" onClick={() => void pickExecutable()}>
              浏览…
            </button>
          </div>
        </label>

        <label className="field">
          <span className="field-label">Config（可选，缺省用默认路径）</span>
          <div className="field-row">
            <input
              className="input mono"
              value={connectOptions.config}
              onChange={(e) => setConnectOptions({ config: e.target.value })}
              placeholder="config.toml"
              spellCheck={false}
            />
            <button type="button" className="btn btn-quiet" onClick={() => void pickConfig()}>
              浏览…
            </button>
          </div>
        </label>

        {lastError && (
          <div className="error-box">
            <span className="mono">{lastError.code}</span>
            <span>{lastError.message}</span>
          </div>
        )}

        <button
          type="button"
          className="btn btn-accent btn-block"
          disabled={busy || !connectOptions.workspace.trim()}
          onClick={() => void connect()}
        >
          {busy ? "连接中…" : "连接并开始对话"}
        </button>

        <p className="muted small">
          连接后 Hello 会返回 server info 与初始 snapshot；随后可用预置 prompt 发起只读快速示范。
        </p>
      </div>
    </main>
  );
}
