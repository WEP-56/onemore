// 关于:版本、协议信息。

import { useStore } from "@/app/store";

export default function AboutSection() {
  const server = useStore((s) => s.server);
  const conn = useStore((s) => s.conn);
  const metrics = useStore((s) => s.metrics);

  const items: { label: string; value: string }[] = [
    { label: "应用", value: "OnemoreGui" },
    { label: "连接状态", value: conn },
    { label: "协议版本", value: server ? String(server.protocol_version) : "—" },
    { label: "Server ID", value: server?.server_id ?? "—" },
    { label: "模型数量", value: server ? String(server.models.length) : "—" },
    { label: "本次会话事件数", value: String(metrics.events) },
    { label: "助手流式字符", value: String(metrics.assistantDeltaChars) },
    { label: "工具调用", value: `${metrics.toolsFinished}/${metrics.toolsStarted}` },
  ];

  return (
    <div>
      <div className="settings-section">
        <div className="flex flex-col items-center gap-2 py-6">
          <span
            className="inline-block h-10 w-10 rounded-full"
            style={{ background: "var(--status-success)", boxShadow: "0 0 24px var(--status-success)" }}
          />
          <h2 className="m-0 text-lg font-semibold">OnemoreGui</h2>
          <p className="m-0 text-[12.5px] text-[var(--text-faint)]">基于 Tauri 2 + React 19 的 Onemore Coding Agent 桌面客户端</p>
        </div>
        <div className="settings-card">
          {items.map((item) => (
            <div key={item.label} className="settings-row" style={{ padding: "8px 0" }}>
              <div className="settings-row-label" style={{ fontSize: 12.5 }}>{item.label}</div>
              <span className="mono text-[12px] text-[var(--text-muted)]">{item.value}</span>
            </div>
          ))}
        </div>
        <p className="settings-section-desc" style={{ textAlign: "center" }}>
          RPC 协议与 onemore CLI 保持一致 · 配置存储于 %APPDATA%/onemore
        </p>
      </div>
    </div>
  );
}
