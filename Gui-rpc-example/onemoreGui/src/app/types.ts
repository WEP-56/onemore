// GUI 本地状态类型（与协议 view types 分离，前端专用）。

import type {
  ApprovalRequestView,
  CommandErrorView,
  CommandStatus,
  ServerInfo,
  SessionPhase,
  SessionSnapshot,
} from "../rpc/protocol";

export type ConnState =
  | "disconnected"
  | "spawning"
  | "handshaking"
  | "connected"
  | "shutting_down";

/// 流式增量组装：一条 assistant 消息按 (message_id, content_index, kind) 累积 delta。
export interface LiveStream {
  key: string;
  messageId: string;
  contentIndex: number;
  kind: string;
  text: string;
  sealed: boolean;
  updatedAt: number;
}

export interface LiveTool {
  toolCallId: string;
  name: string;
  summary: string;
  output: string;
  status: "started" | "updated" | "finished";
  error: string | null;
  sealed: boolean;
  updatedAt: number;
}

export interface LiveUserMessage {
  key: string;
  text: string;
  at: number;
}

export interface LiveNotice {
  key: string;
  level: "info" | "warning" | "error";
  text: string;
  at: number;
}

export interface RunMetrics {
  startedAt: number | null;
  endedAt: number | null;
  events: number;
  snapshots: number;
  progresses: number;
  commandFinished: number;
  settled: number;
  assistantDeltaChars: number;
  toolsStarted: number;
  toolsFinished: number;
  acceptedCommands: number;
  terminals: number;
  maxQueue: number;
  phaseMs: Partial<Record<SessionPhase, number>>;
  lastPhase: SessionPhase | null;
  lastPhaseAt: number | null;
}

export interface TerminalRecord {
  commandId: string;
  status: CommandStatus;
  error: CommandErrorView | null;
  at: number;
}

export interface TransportIssue {
  code: string;
  message: string;
  at: number;
}

export interface SessionViewState {
  conn: ConnState;
  server: ServerInfo | null;
  snapshot: SessionSnapshot | null;
  liveStreams: Record<string, LiveStream>;
  liveTools: Record<string, LiveTool>;
  liveUsers: Record<string, LiveUserMessage>;
  liveNotices: LiveNotice[];
  liveApproval: ApprovalRequestView | null;
  run: { commandId: string | null; startedAt: number | null };
  lastTerminal: TerminalRecord | null;
  lastError: { code: string; message: string } | null;
  stderrLines: string[];
  transportIssues: TransportIssue[];
  metrics: RunMetrics;
}

// ── 后端补充类型（非 RPC 协议）──

export interface WorkspaceEntry {
  path: string;
  label: string;
  last_used: number;
}

export interface WorkspaceList {
  workspaces: WorkspaceEntry[];
}

export interface SessionEntry {
  id: string;
  workspace: string;
  title: string;
  created_at: number;
  updated_at: number;
  input_tokens: number;
  output_tokens: number;
  message_count: number;
}

export interface GitFileStatus {
  path: string;
  status: string;
  staged: boolean;
}

export interface GitStatus {
  branch: string;
  ahead: number;
  behind: number;
  files: GitFileStatus[];
  is_repo: boolean;
}

export interface FileTreeNode {
  name: string;
  path: string;
  is_dir: boolean;
  children: FileTreeNode[];
}
