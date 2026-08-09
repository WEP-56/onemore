// 通用小工具：ID 缩短、token/duration 格式化、复制。

export function shortId(id: string, len = 10): string {
  if (id.length <= len) return id;
  return `${id.slice(0, len)}…`;
}

export function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
}

export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) ms = 0;
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  if (h > 0) return `${h}h ${m}m ${sec}s`;
  if (m > 0) return `${m}m ${sec}s`;
  return `${sec}s`;
}

export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

export function phaseLabel(phase: string): string {
  const map: Record<string, string> = {
    idle: "空闲",
    running: "运行中",
    retrying: "重试",
    compacting: "压缩中",
    waiting_approval: "等待审批",
    shutting_down: "关闭中",
  };
  return map[phase] ?? phase;
}

/// 规范化 workspace 路径：去掉 UNC 前缀 \\?\，统一为正常路径。
export function normalizeWorkspace(path: string): string {
  return path.startsWith("\\\\?\\") ? path.slice(4) : path;
}

/** 用于关联 SQLite 会话与 GUI 工作区的稳定键。 */
export function workspaceKey(path: string): string {
  const normalized = normalizeWorkspace(path).replace(/\//g, "\\").replace(/\\+$/, "");
  return /^[a-z]:\\/i.test(normalized) ? normalized.toLocaleLowerCase() : normalized;
}

/// 格式化时间戳（秒）为简短的相对时间。
export function relativeTime(ts: number): string {
  const now = Date.now();
  const diff = now - ts * 1000;
  if (diff < 60_000) return "刚刚";
  if (diff < 3600_000) return `${Math.floor(diff / 60_000)}分钟前`;
  if (diff < 86400_000) return `${Math.floor(diff / 3600_000)}小时前`;
  if (diff < 604800_000) return `${Math.floor(diff / 86400_000)}天前`;
  return new Date(ts * 1000).toLocaleDateString();
}
