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
  group_id?: string | null;
}

export interface WorkspaceGroup {
  id: string;
  name: string;
}

export interface WorkspaceList {
  workspaces: WorkspaceEntry[];
  groups?: WorkspaceGroup[];
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

// ── config.toml DTO(与 src-tauri/src/config_edit.rs 对齐)──

export interface AgentDto {
  provider: string;
  shell: string;
  max_turns: number | null;
  tool_timeout_secs: number | null;
  system_prompt: string | null;
}

export interface RetryDto {
  max_attempts: number | null;
  base_delay_ms: number | null;
  max_delay_ms: number | null;
  max_retry_after_ms: number | null;
}

export interface CompactionDto {
  enabled: boolean | null;
  reserve_tokens: number | null;
  keep_recent_tokens: number | null;
}

export interface PermissionsDto {
  workspace_read: string | null;
  workspace_write: string | null;
  outside_workspace: string | null;
  commands: string | null;
}

export interface ModelDto {
  name: string;
  context_window: number | null;
  max_tokens: number | null;
  efforts: string[];
  default_effort: string | null;
}

export interface ProviderDto {
  name: string;
  api: string;
  profile: string | null;
  base_url: string;
  api_key_env: string | null;
  api_key: string | null;
  default_model: string | null;
  models: ModelDto[];
}

export interface ConfigDto {
  agent: AgentDto;
  retry: RetryDto;
  compaction: CompactionDto;
  permissions: PermissionsDto;
  providers: ProviderDto[];
}
