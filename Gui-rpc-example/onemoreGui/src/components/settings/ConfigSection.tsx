// 配置:可视化调整 config.toml(表单化)+ 原始 TOML 编辑。
// 表单字段对齐 onemore CLI 的 FileConfig 结构。

import { useEffect, useMemo, useState } from "react";
import { useStore } from "@/app/store";
import {
  Check,
  ChevronDown,
  Copy,
  Plus,
  Save,
  Trash2,
  Wrench,
} from "lucide-react";
import type { ConfigDto, ModelDto, ProviderDto } from "@/app/types";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";

const SHELL_OPTIONS = ["auto", "gitbash", "powershell", "cmd"];
const RULE_OPTIONS = ["allow", "ask", "deny"];

function emptyProvider(): ProviderDto {
  return {
    name: "",
    api: "messages",
    profile: null,
    base_url: "",
    api_key_env: null,
    api_key: null,
    default_model: null,
    max_tokens: null,
    context_window: null,
    models: [],
  };
}

function emptyModel(): ModelDto {
  return { name: "", context_window: null, max_tokens: null, efforts: [], default_effort: null };
}

export default function ConfigSection() {
  const [tab, setTab] = useState<"visual" | "raw">("visual");
  return (
    <div>
      <div className="settings-tabbar">
        <button type="button" className={cn("settings-tab", tab === "visual" && "is-active")} onClick={() => setTab("visual")}>
          可视化编辑
        </button>
        <button type="button" className={cn("settings-tab", tab === "raw" && "is-active")} onClick={() => setTab("raw")}>
          原始 TOML
        </button>
      </div>
      {tab === "visual" ? <VisualConfig /> : <RawConfig />}
    </div>
  );
}

/* ── 可视化编辑 ── */
function VisualConfig() {
  const dto = useStore((s) => s.configDto);
  const loadConfigDto = useStore((s) => s.loadConfigDto);
  const saveConfigDto = useStore((s) => s.saveConfigDto);
  const [draft, setDraft] = useState<ConfigDto | null>(null);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    if (!dto) void loadConfigDto();
    else setDraft(structuredClone(dto));
  }, [dto, loadConfigDto]);

  const dirty = useMemo(() => {
    if (!draft || !dto) return false;
    return JSON.stringify(draft) !== JSON.stringify(dto);
  }, [draft, dto]);

  if (!draft) return <div className="settings-empty">加载配置中…</div>;

  const patch = (fn: (d: ConfigDto) => void) => {
    const next = structuredClone(draft);
    fn(next);
    setDraft(next);
  };

  const handleSave = async () => {
    setSaving(true);
    await saveConfigDto(draft);
    setSaving(false);
    setSaved(true);
    window.setTimeout(() => setSaved(false), 1800);
  };

  return (
    <div>
      <div className="settings-section">
        <h3 className="settings-section-title">Agent</h3>
        <p className="settings-section-desc">当前生效的提供商与运行参数。</p>
        <div className="settings-card">
          <div className="settings-grid-2">
            <div className="settings-field">
              <Label className="settings-field-label">提供商 (provider)</Label>
              <Input
                value={draft.agent.provider}
                onChange={(e) => patch((d) => { d.agent.provider = e.target.value; })}
                spellCheck={false}
              />
            </div>
            <div className="settings-field">
              <Label className="settings-field-label">Shell</Label>
              <select
                className="h-8 w-full rounded-md border px-2 text-[12.5px] outline-none"
                style={{ borderColor: "var(--border-strong)", background: "var(--surface-control)", color: "var(--text-primary)" }}
                value={draft.agent.shell}
                onChange={(e) => patch((d) => { d.agent.shell = e.target.value; })}
              >
                {SHELL_OPTIONS.map((s) => <option key={s} value={s}>{s}</option>)}
              </select>
            </div>
            <div className="settings-field">
              <Label className="settings-field-label">最大连续调用次数 (max_turns)</Label>
              <Input
                type="number"
                min={1}
                value={draft.agent.max_turns ?? ""}
                placeholder="200"
                onChange={(e) => patch((d) => { d.agent.max_turns = e.target.value ? Number(e.target.value) : null; })}
              />
            </div>
            <div className="settings-field">
              <Label className="settings-field-label">工具超时秒数 (tool_timeout_secs)</Label>
              <Input
                type="number"
                min={0}
                value={draft.agent.tool_timeout_secs ?? ""}
                placeholder="0 = 不限制"
                onChange={(e) => patch((d) => { d.agent.tool_timeout_secs = e.target.value ? Number(e.target.value) : null; })}
              />
            </div>
          </div>
          <div className="settings-field" style={{ marginTop: 10 }}>
            <Label className="settings-field-label">系统提示词 (system_prompt,可选)</Label>
            <Textarea
              rows={3}
              value={draft.agent.system_prompt ?? ""}
              placeholder="留空使用默认系统提示"
              onChange={(e) => patch((d) => { d.agent.system_prompt = e.target.value ? e.target.value : null; })}
            />
          </div>
        </div>
      </div>

      <div className="settings-section">
        <h3 className="settings-section-title">重试 (retry)</h3>
        <div className="settings-card">
          <div className="settings-grid-2">
            <div className="settings-field">
              <Label className="settings-field-label">最大尝试次数</Label>
              <Input type="number" min={1} value={draft.retry.max_attempts ?? ""} placeholder="8" onChange={(e) => patch((d) => { d.retry.max_attempts = e.target.value ? Number(e.target.value) : null; })} />
            </div>
            <div className="settings-field">
              <Label className="settings-field-label">初始延迟 (ms)</Label>
              <Input type="number" min={0} value={draft.retry.base_delay_ms ?? ""} placeholder="1000" onChange={(e) => patch((d) => { d.retry.base_delay_ms = e.target.value ? Number(e.target.value) : null; })} />
            </div>
            <div className="settings-field">
              <Label className="settings-field-label">最大延迟 (ms)</Label>
              <Input type="number" min={0} value={draft.retry.max_delay_ms ?? ""} placeholder="10000" onChange={(e) => patch((d) => { d.retry.max_delay_ms = e.target.value ? Number(e.target.value) : null; })} />
            </div>
            <div className="settings-field">
              <Label className="settings-field-label">最大 Retry-After (ms)</Label>
              <Input type="number" min={0} value={draft.retry.max_retry_after_ms ?? ""} placeholder="60000" onChange={(e) => patch((d) => { d.retry.max_retry_after_ms = e.target.value ? Number(e.target.value) : null; })} />
            </div>
          </div>
        </div>
      </div>

      <div className="settings-section">
        <h3 className="settings-section-title">自动压缩 (compaction)</h3>
        <div className="settings-card">
          <div className="settings-row" style={{ padding: "0 0 10px" }}>
            <div>
              <div className="settings-row-label">启用自动压缩</div>
              <div className="settings-row-hint">接近上下文上限前自动压缩历史</div>
            </div>
            <Switch
              checked={draft.compaction.enabled ?? false}
              onCheckedChange={(v) => patch((d) => { d.compaction.enabled = v; })}
            />
          </div>
          <div className="settings-grid-2">
            <div className="settings-field">
              <Label className="settings-field-label">预留余量 (reserve_tokens)</Label>
              <Input type="number" min={0} value={draft.compaction.reserve_tokens ?? ""} placeholder="16384" onChange={(e) => patch((d) => { d.compaction.reserve_tokens = e.target.value ? Number(e.target.value) : null; })} />
            </div>
            <div className="settings-field">
              <Label className="settings-field-label">保留最近消息 (keep_recent_tokens)</Label>
              <Input type="number" min={0} value={draft.compaction.keep_recent_tokens ?? ""} placeholder="20000" onChange={(e) => patch((d) => { d.compaction.keep_recent_tokens = e.target.value ? Number(e.target.value) : null; })} />
            </div>
          </div>
        </div>
      </div>

      <div className="settings-section">
        <h3 className="settings-section-title">权限 (permissions)</h3>
        <p className="settings-section-desc">权限规则:allow | ask | deny。hard deny 不受此处覆盖。</p>
        <div className="settings-card">
          <div className="settings-grid-2">
            {(Object.keys(draft.permissions) as (keyof ConfigDto["permissions"])[]).map((key) => (
              <div className="settings-field" key={key}>
                <Label className="settings-field-label">{key}</Label>
                <select
                  className="h-8 w-full rounded-md border px-2 text-[12.5px] outline-none"
                  style={{ borderColor: "var(--border-strong)", background: "var(--surface-control)", color: "var(--text-primary)" }}
                  value={draft.permissions[key] ?? "allow"}
                  onChange={(e) => patch((d) => { d.permissions[key] = e.target.value; })}
                >
                  {RULE_OPTIONS.map((r) => <option key={r} value={r}>{r}</option>)}
                </select>
              </div>
            ))}
          </div>
        </div>
      </div>

      <div className="settings-section">
        <h3 className="settings-section-title">提供商 (providers)</h3>
        <p className="settings-section-desc">每个提供商包含 API 配置与其模型列表。</p>
        <ProvidersEditor
          providers={draft.providers}
          onChange={(providers) => patch((d) => { d.providers = providers; })}
        />
      </div>

      <div className="flex items-center gap-3">
        <Button onClick={() => void handleSave()} disabled={!dirty || saving}>
          {saved ? <Check size={14} /> : <Save size={14} />}
          {saved ? "已保存" : "保存配置"}
        </Button>
        {!dirty && <span className="text-[11.5px] text-[var(--text-faint)]">暂无更改</span>}
      </div>
    </div>
  );
}

/* ── 提供商编辑 ── */
function ProvidersEditor({ providers, onChange }: { providers: ProviderDto[]; onChange: (p: ProviderDto[]) => void }) {
  const [activeName, setActiveName] = useState<string | null>(providers[0]?.name ?? null);
  const active = providers.find((p) => p.name === activeName) ?? providers[0] ?? null;

  const updateProvider = (name: string, fn: (p: ProviderDto) => void) => {
    onChange(providers.map((p) => {
      if (p.name !== name) return p;
      const next = structuredClone(p);
      fn(next);
      return next;
    }));
  };

  const addProvider = () => {
    const p = emptyProvider();
    p.name = `provider-${providers.length + 1}`;
    onChange([...providers, p]);
    setActiveName(p.name);
  };

  const removeProvider = (name: string) => {
    const next = providers.filter((p) => p.name !== name);
    onChange(next);
    setActiveName(next[0]?.name ?? null);
  };

  const updateModel = (providerName: string, modelName: string, fn: (m: ModelDto) => void) => {
    updateProvider(providerName, (p) => {
      p.models = p.models.map((m) => {
        if (m.name !== modelName) return m;
        const next = structuredClone(m);
        fn(next);
        return next;
      });
    });
  };

  const addModel = (providerName: string) => {
    updateProvider(providerName, (p) => {
      const m = emptyModel();
      m.name = `model-${p.models.length + 1}`;
      p.models = [...p.models, m];
    });
  };

  const removeModel = (providerName: string, modelName: string) => {
    updateProvider(providerName, (p) => {
      p.models = p.models.filter((m) => m.name !== modelName);
    });
  };

  return (
    <div>
      <div className="settings-card" style={{ padding: 8 }}>
        {providers.length === 0 && (
          <div className="settings-empty" style={{ padding: "12px" }}>尚未配置提供商</div>
        )}
        {providers.map((p) => (
          <div
            key={p.name}
            className={cn("settings-provider-row", active?.name === p.name && "is-active")}
            onClick={() => setActiveName(p.name)}
          >
            <Wrench size={14} className="shrink-0 text-[var(--text-faint)]" />
            <div className="settings-provider-main">
              <div className="settings-provider-name">{p.name}</div>
              <div className="settings-provider-meta">
                {p.api} · {p.base_url || "未设置 base_url"} · {p.models.length} 个模型
              </div>
            </div>
            <span className="settings-provider-actions">
              <button
                type="button"
                className="sidebar-icon-btn danger"
                title="删除提供商"
                onClick={(e) => {
                  e.stopPropagation();
                  removeProvider(p.name);
                }}
              >
                <Trash2 size={13} />
              </button>
            </span>
          </div>
        ))}
      </div>

      {active && (
        <div className="settings-card">
          <div className="settings-card-header">
            <span className="settings-card-title">{active.name}</span>
            <Button variant="ghost" size="icon-sm" title="复制提供商配置" onClick={() => void navigator.clipboard.writeText(JSON.stringify(active, null, 2))}>
              <Copy size={13} />
            </Button>
          </div>
          <div className="settings-grid-2">
            <div className="settings-field">
              <Label className="settings-field-label">名称</Label>
              <Input value={active.name} spellCheck={false} onChange={(e) => {
                const old = active.name;
                updateProvider(old, (p) => { p.name = e.target.value; });
                setActiveName(e.target.value);
              }} />
            </div>
            <div className="settings-field">
              <Label className="settings-field-label">API</Label>
              <Input value={active.api} spellCheck={false} onChange={(e) => updateProvider(active.name, (p) => { p.api = e.target.value; })} />
            </div>
            <div className="settings-field">
              <Label className="settings-field-label">Profile</Label>
              <Input value={active.profile ?? ""} placeholder="anthropic / openai / …" spellCheck={false} onChange={(e) => updateProvider(active.name, (p) => { p.profile = e.target.value || null; })} />
            </div>
            <div className="settings-field">
              <Label className="settings-field-label">Base URL</Label>
              <Input value={active.base_url} spellCheck={false} onChange={(e) => updateProvider(active.name, (p) => { p.base_url = e.target.value; })} />
            </div>
            <div className="settings-field">
              <Label className="settings-field-label">API Key 环境变量</Label>
              <Input value={active.api_key_env ?? ""} spellCheck={false} onChange={(e) => updateProvider(active.name, (p) => { p.api_key_env = e.target.value || null; })} />
            </div>
            <div className="settings-field">
              <Label className="settings-field-label">API Key(直接填写)</Label>
              <Input type="password" value={active.api_key ?? ""} spellCheck={false} onChange={(e) => updateProvider(active.name, (p) => { p.api_key = e.target.value || null; })} />
            </div>
            <div className="settings-field">
              <Label className="settings-field-label">默认模型</Label>
              <Input value={active.default_model ?? ""} spellCheck={false} onChange={(e) => updateProvider(active.name, (p) => { p.default_model = e.target.value || null; })} />
            </div>
          </div>

          <div style={{ marginTop: 14 }}>
            <div className="settings-card-header">
              <span className="settings-card-title">模型 ({active.models.length})</span>
              <Button variant="outline" size="sm" onClick={() => addModel(active.name)}>
                <Plus size={13} /> 添加模型
              </Button>
            </div>
            {active.models.length === 0 && <div className="settings-empty" style={{ padding: "10px" }}>暂无模型</div>}
            {active.models.map((m) => (
              <div key={m.name} className="settings-card" style={{ padding: "10px 12px" }}>
                <div className="settings-card-header">
                  <Input
                    className="h-7 w-56 text-[12.5px] font-medium"
                    value={m.name}
                    spellCheck={false}
                    onChange={(e) => updateModel(active.name, m.name, (mm) => { mm.name = e.target.value; })}
                  />
                  <Button variant="ghost" size="icon-sm" title="删除模型" onClick={() => removeModel(active.name, m.name)}>
                    <Trash2 size={13} className="text-[var(--status-error)]" />
                  </Button>
                </div>
                <div className="settings-grid-2">
                  <div className="settings-field">
                    <Label className="settings-field-label">Context Window</Label>
                    <Input type="number" min={0} value={m.context_window ?? ""} onChange={(e) => updateModel(active.name, m.name, (mm) => { mm.context_window = e.target.value ? Number(e.target.value) : null; })} />
                  </div>
                  <div className="settings-field">
                    <Label className="settings-field-label">Max Tokens</Label>
                    <Input type="number" min={0} value={m.max_tokens ?? ""} onChange={(e) => updateModel(active.name, m.name, (mm) => { mm.max_tokens = e.target.value ? Number(e.target.value) : null; })} />
                  </div>
                  <div className="settings-field">
                    <Label className="settings-field-label">Efforts(逗号分隔)</Label>
                    <Input value={m.efforts.join(", ")} spellCheck={false} onChange={(e) => updateModel(active.name, m.name, (mm) => { mm.efforts = e.target.value.split(",").map((s) => s.trim()).filter(Boolean); })} />
                  </div>
                  <div className="settings-field">
                    <Label className="settings-field-label">默认 Effort</Label>
                    <Input value={m.default_effort ?? ""} spellCheck={false} onChange={(e) => updateModel(active.name, m.name, (mm) => { mm.default_effort = e.target.value || null; })} />
                  </div>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      <Button variant="outline" size="sm" onClick={addProvider}>
        <Plus size={13} /> 添加提供商
      </Button>
    </div>
  );
}

/* ── 原始 TOML 编辑 ── */
function RawConfig() {
  const configText = useStore((s) => s.configText);
  const loadConfig = useStore((s) => s.loadConfig);
  const saveConfig = useStore((s) => s.saveConfig);
  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    if (!loaded) {
      void loadConfig();
      setLoaded(true);
    }
  }, [loaded, loadConfig]);

  useEffect(() => {
    if (loaded) setDraft(configText);
  }, [configText, loaded]);

  const dirty = draft !== configText;

  return (
    <div>
      <div className="settings-section">
        <p className="settings-section-desc">
          直接编辑 config.toml(保留全部注释)。保存后新会话生效。可视化编辑会重写部分结构并可能丢失注释,建议优先使用此模式保留自定义内容。
        </p>
        <Textarea
          className="mono h-[420px] resize-none text-[12.5px]"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          spellCheck={false}
          style={{ lineHeight: 1.55 }}
        />
        <div className="mt-3 flex items-center gap-3">
          <Button onClick={() => { setSaving(true); void saveConfig(draft).then(() => setSaving(false)); }} disabled={!dirty || saving}>
            <Save size={14} /> 保存
          </Button>
          <Button variant="ghost" size="sm" onClick={() => { setDraft(configText); }}>
            放弃更改
          </Button>
          {!dirty && <span className="text-[11.5px] text-[var(--text-faint)]">暂无更改</span>}
        </div>
      </div>
      <div className="flex items-center gap-2 text-[11.5px] text-[var(--text-faint)]">
        <ChevronDown size={12} className="rotate-180" />
        配置文件路径: %APPDATA%/onemore/config.toml
      </div>
    </div>
  );
}
