// 顶栏模型选择器:provider 分组 → model → effort。参照 cc-gui ChatInputBox 模型选择交互。

import { Check, ChevronDown, Sparkles } from "lucide-react";
import { useStore } from "@/app/store";
import { cn } from "@/lib/utils";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

export function ModelSelectMenu() {
  const server = useStore((s) => s.server);
  const snapshot = useStore((s) => s.snapshot);
  const setModel = useStore((s) => s.setModel);
  const conn = useStore((s) => s.conn);

  const models = server?.models ?? [];
  const current = snapshot?.model ?? null;
  const connected = conn === "connected";

  if (!connected || models.length === 0) return null;

  // 按 provider 分组
  const groups = new Map<string, typeof models>();
  for (const m of models) {
    const list = groups.get(m.provider) ?? [];
    list.push(m);
    groups.set(m.provider, list);
  }

  const currentLabel = current?.label ?? (current ? `${current.provider}/${current.model}` : null);

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="flex h-7 max-w-[220px] items-center gap-1.5 rounded-md px-2 text-[12px] text-[var(--text-muted)] transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--text-strong)]"
          title="切换模型"
        >
          <Sparkles size={13} className="shrink-0 text-[var(--status-success)]" />
          <span className="truncate">{currentLabel ?? "选择模型"}</span>
          <ChevronDown size={12} className="shrink-0 text-[var(--text-faint)]" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-64">
        {[...groups.entries()].map(([provider, providerModels], gi) => (
          <DropdownMenuGroup key={provider}>
            {gi > 0 && <DropdownMenuSeparator />}
            <DropdownMenuLabel className="text-[11px] uppercase tracking-wider text-[var(--text-faint)]">
              {provider}
            </DropdownMenuLabel>
            {providerModels.map((m) => {
              const isCurrentModel = current?.provider === m.provider && current?.model === m.model;
              const efforts = m.supported_efforts ?? [];
              const canPickEffort = isCurrentModel && efforts.length > 1;
              const item = (
                <DropdownMenuItem
                  key={m.model}
                  className={cn("flex items-center gap-2", canPickEffort && "pr-2")}
                  onSelect={(e) => {
                    if (canPickEffort) e.preventDefault();
                    else void setModel(m.provider, m.model, m.default_effort ?? "");
                  }}
                >
                  {isCurrentModel ? <Check size={13} className="text-[var(--status-success)]" /> : <span className="w-[13px]" />}
                  <span className="flex min-w-0 flex-1 flex-col">
                    <span className="truncate">{m.label}</span>
                    <span className="mono text-[10px] text-[var(--text-faint)]">{m.model}</span>
                  </span>
                </DropdownMenuItem>
              );
              if (!canPickEffort) return item;
              return (
                <DropdownMenuSub key={m.model}>
                  <DropdownMenuSubTrigger className="flex items-center gap-2">
                    <Check size={13} className="text-[var(--status-success)]" />
                    <span className="flex min-w-0 flex-1 flex-col">
                      <span className="truncate">{m.label}</span>
                      <span className="mono text-[10px] text-[var(--text-faint)]">{m.model}</span>
                    </span>
                  </DropdownMenuSubTrigger>
                  <DropdownMenuSubContent className="w-40">
                    <DropdownMenuLabel className="text-[11px] uppercase tracking-wider text-[var(--text-faint)]">
                      Effort
                    </DropdownMenuLabel>
                    {efforts.map((effort) => (
                      <DropdownMenuItem
                        key={effort}
                        onSelect={() => void setModel(m.provider, m.model, effort)}
                      >
                        {current?.effort === effort && <Check size={13} className="text-[var(--status-success)]" />}
                        <span className={cn(current?.effort === effort && "pl-0", current?.effort !== effort && "pl-[13px]")}>
                          {effort}
                        </span>
                      </DropdownMenuItem>
                    ))}
                  </DropdownMenuSubContent>
                </DropdownMenuSub>
              );
            })}
          </DropdownMenuGroup>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
