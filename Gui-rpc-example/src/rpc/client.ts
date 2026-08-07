// Tauri backend 的唯一调用入口。前端不接触 Child/pipe/路径，只调用 typed commands
// 并消费单一事件流 `onemore://rpc-event`。

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ServerInfo, SessionEvent, SessionSnapshot } from "./protocol";

export interface StartOptions {
  executable: string;
  config: string | null;
  workspace: string;
}

export interface GuiErrorDto {
  code: string;
  message: string;
}

/// backend 发送的封闭事件 DTO（RpcEvent 的序列化形状）。
export type BackendEvent =
  | { kind: "hello"; server: ServerInfo; snapshot: SessionSnapshot }
  | { kind: "event"; event: SessionEvent }
  | { kind: "stderr"; line: string }
  | { kind: "process_exit"; code: number | null }
  | { kind: "transport_error"; code: string; message: string };

const EVENT_NAME = "onemore://rpc-event";

export function subscribeBackend(handler: (e: BackendEvent) => void): Promise<() => void> {
  return listen<BackendEvent>(EVENT_NAME, (ev) => handler(ev.payload));
}

export async function rpcStart(options: StartOptions): Promise<void> {
  await invoke("rpc_start", { options });
}

export async function rpcRequest<T = unknown>(
  command: string,
  params?: Record<string, unknown>,
): Promise<T> {
  return invoke<T>("rpc_request", { command, params: params ?? null });
}

export async function rpcStop(): Promise<void> {
  await invoke("rpc_stop");
}

export async function rpcDiagnosticsTail(limit: number): Promise<string[]> {
  return invoke<string[]>("rpc_diagnostics_tail", { limit });
}

/// 最近一次权威 snapshot（前端热重载后恢复画面）。
export async function rpcSnapshot(): Promise<SessionSnapshot | null> {
  return invoke<SessionSnapshot | null>("rpc_snapshot");
}

export function toErrorMessage(e: unknown): GuiErrorDto {
  if (e && typeof e === "object" && "code" in e && "message" in e) {
    const g = e as GuiErrorDto;
    return { code: String(g.code), message: String(g.message) };
  }
  return { code: "unknown", message: String(e) };
}
