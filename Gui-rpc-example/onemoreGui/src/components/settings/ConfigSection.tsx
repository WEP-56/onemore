// 配置：可视化调整 config.toml（表单化）+ 原始 TOML 编辑。
// 表单字段对齐 onemore CLI 的 FileConfig 结构。
import { useEffect, useMemo, useState } from "react";
import { useStore } from "@/app/store";
import {
  Check,
  ChevronDown,
  ChevronRight,
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
const RULE_OPTIONS = [
  { value: "allow", label: "允许" },
  { value: "ask", label: "每次询问" },
  { value: "deny", label: "拒绝" },
];
const PERMISSION_FIELDS: Array<{
  key: keyof ConfigDto["permissions"];
  label: string;
  hint: string;
}> = [
  { key: "workspace_read", label: "工作区读取", hint: "读取当前工作区内的文件" },
  { key: "workspace_write", label: "工作区写入", hint: "创建、修改或删除工作区内的文件" },
  { key: "outside_workspace", label: "工作区外访问", hint: "访问当前工作区之外的路径" },
  { key: "commands", label: "命令执行", hint: "在本机 Shell 中运行命令" },
];

const SELECT_CLASS = "config-select";

function emptyProvider(): ProviderDto {
  return {
    name: "",
    api: "messages",
    profile: null,
    base_url: "",
    api_key_env: null,
    api_key: null,
    default_model: null,
    models: [],
  };
}

function emptyModel(): ModelDto {
  return {
    name: "",
    context_window: null,
    max_tokens: null,
    efforts: [],
    default_effort: null,
  };
}

function nextAvailableName(prefix: string, names: string[]) {
  let suffix = names.length + 1;
  let candidate = `${prefix}-${suffix}`;
  while (names.includes(candidate)) {
    suffix += 1;
    candidate = `${prefix}-${suffix}`;
  }
  return candidate;
}

function configValidation(draft: ConfigDto): string | null {
  if (draft.providers.length === 0) return "至少需要配置一个供应商";
  const providerNames = draft.providers.map((provider) => provider.name.trim());
  if (providerNames.some((name) => !name)) return "供应商名称不能为空";
  if (new Set(providerNames).size !== providerNames.length) return "供应商名称不能重复";
  if (draft.providers.length > 0 && !providerNames.includes(draft.agent.provider)) {
    return "首选供应商必须来自供应商列表";
  }

  for (const provider of draft.providers) {
    const modelNames = provider.models.map((model) => model.name.trim());
    if (modelNames.length === 0) return `${provider.name} 至少需要配置一个模型`;
    if (modelNames.some((name) => !name)) return `${provider.name} 中的模型名称不能为空`;
    if (new Set(modelNames).size !== modelNames.length) return `${provider.name} 中的模型名称不能重复`;
    if (!provider.default_model || !modelNames.includes(provider.default_model)) {
      return `${provider.name} 的默认模型必须来自模型列表`;
    }
    for (const model of provider.models) {
      if (!model.context_window || model.context_window <= 0) {
        return `${provider.name} / ${model.name} 需要有效的 Context Window`;
      }
      if (model.max_tokens != null && model.max_tokens <= 0) {
        return `${provider.name} / ${model.name} 的 Max Tokens 必须大于 0`;
      }
      if (model.max_tokens != null && model.max_tokens > model.context_window) {
        return `${provider.name} / ${model.name} 的 Max Tokens 不能超过 Context Window`;
      }
    }
  }
  return null;
}

export default function ConfigSection() {
  const [tab, setTab] = useState<"visual" | "raw">("visual");

  return (
    <div>
      <div className="settings-tabbar">
        <button
          type="button"
          className={cn("settings-tab", tab === "visual" && "is-active")}
          onClick={() => setTab("visual")}
        >
          可视化编辑
        </button>
        <button
          type="button"
          className={cn("settings-tab", tab === "raw" && "is-active")}
          onClick={() => setTab("raw")}
        >
          原始 TOML
        </button>
      </div>
      {tab === "visual" ? <VisualConfig /> : <RawConfig />}
    </div>
  );
}

function VisualConfig() {
  const dto = useStore((state) => state.configDto);
  const loadConfigDto = useStore((state) => state.loadConfigDto);
  const saveConfigDto = useStore((state) => state.saveConfigDto);
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

  const validationIssue = useMemo(
    () => (draft ? configValidation(draft) : null),
    [draft],
  );

  if (!draft) return <div className="settings-empty">加载配置中...</div>;

  const patch = (fn: (next: ConfigDto) => void) => {
    const next = structuredClone(draft);
    fn(next);
    setDraft(next);
    setSaved(false);
  };

  const handleSave = async () => {
    if (validationIssue) return;
    setSaving(true);
    try {
      await saveConfigDto(draft);
      setSaved(true);
      window.setTimeout(() => setSaved(false), 1800);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="config-visual">
      <section className="settings-section">
        <h3 className="settings-section-title">供应商</h3>
        <p className="settings-section-desc">
          管理模型服务、鉴权方式和各供应商可用的模型。点击标题可展开或收起。
        </p>
        <ProvidersEditor
          providers={draft.providers}
          activeProvider={draft.agent.provider}
          onChange={(providers, activeProvider) => patch((next) => {
            next.providers = providers;
            next.agent.provider = activeProvider;
          })}
        />
      </section>

      <section className="settings-section">
        <h3 className="settings-section-title">首选项</h3>
        <p className="settings-section-desc">设置新会话默认使用的供应商和 Agent 运行参数。</p>
        <div className="config-section-surface">
          <div className="settings-grid-2">
            <div className="settings-field">
              <Label className="settings-field-label">首选供应商</Label>
              <select
                className={SELECT_CLASS}
                value={draft.agent.provider}
                onChange={(event) => patch((next) => { next.agent.provider = event.target.value; })}
                disabled={draft.providers.length === 0}
              >
                {draft.providers.length === 0 && <option value="">尚未配置供应商</option>}
                {draft.providers.map((provider, index) => (
                  <option key={`${index}-${provider.name}`} value={provider.name}>
                    {provider.name || "未命名供应商"}
                  </option>
                ))}
              </select>
            </div>
            <div className="settings-field">
              <Label className="settings-field-label">Shell</Label>
              <select
                className={SELECT_CLASS}
                value={draft.agent.shell}
                onChange={(event) => patch((next) => { next.agent.shell = event.target.value; })}
              >
                {SHELL_OPTIONS.map((shell) => <option key={shell} value={shell}>{shell}</option>)}
              </select>
            </div>
            <div className="settings-field">
              <Label className="settings-field-label">最大连续调用次数</Label>
              <Input
                type="number"
                min={1}
                value={draft.agent.max_turns ?? ""}
                placeholder="200"
                onChange={(event) => patch((next) => {
                  next.agent.max_turns = event.target.value ? Number(event.target.value) : null;
                })}
              />
            </div>
            <div className="settings-field">
              <Label className="settings-field-label">工具超时（秒）</Label>
              <Input
                type="number"
                min={0}
                value={draft.agent.tool_timeout_secs ?? ""}
                placeholder="0 表示不限制"
                onChange={(event) => patch((next) => {
                  next.agent.tool_timeout_secs = event.target.value ? Number(event.target.value) : null;
                })}
              />
            </div>
          </div>
          <div className="settings-field config-wide-field">
            <Label className="settings-field-label">系统提示词（可选）</Label>
            <Textarea
              rows={4}
              value={draft.agent.system_prompt ?? ""}
              placeholder="留空使用默认系统提示词"
              onChange={(event) => patch((next) => {
                next.agent.system_prompt = event.target.value || null;
              })}
            />
          </div>
        </div>
      </section>

      <section className="settings-section">
        <h3 className="settings-section-title">运行与重试</h3>
        <p className="settings-section-desc">请求尚未产生流事件时，按以下规则自动重试。</p>
        <div className="config-section-surface">
          <div className="settings-grid-2">
            <div className="settings-field">
              <Label className="settings-field-label">最大尝试次数</Label>
              <Input type="number" min={1} value={draft.retry.max_attempts ?? ""} placeholder="8" onChange={(event) => patch((next) => { next.retry.max_attempts = event.target.value ? Number(event.target.value) : null; })} />
            </div>
            <div className="settings-field">
              <Label className="settings-field-label">初始延迟（毫秒）</Label>
              <Input type="number" min={0} value={draft.retry.base_delay_ms ?? ""} placeholder="1000" onChange={(event) => patch((next) => { next.retry.base_delay_ms = event.target.value ? Number(event.target.value) : null; })} />
            </div>
            <div className="settings-field">
              <Label className="settings-field-label">最大延迟（毫秒）</Label>
              <Input type="number" min={0} value={draft.retry.max_delay_ms ?? ""} placeholder="10000" onChange={(event) => patch((next) => { next.retry.max_delay_ms = event.target.value ? Number(event.target.value) : null; })} />
            </div>
            <div className="settings-field">
              <Label className="settings-field-label">最大 Retry-After（毫秒）</Label>
              <Input type="number" min={0} value={draft.retry.max_retry_after_ms ?? ""} placeholder="60000" onChange={(event) => patch((next) => { next.retry.max_retry_after_ms = event.target.value ? Number(event.target.value) : null; })} />
            </div>
          </div>
        </div>
      </section>

      <section className="settings-section">
        <h3 className="settings-section-title">上下文压缩</h3>
        <p className="settings-section-desc">接近模型上下文上限时压缩较早的历史消息。</p>
        <div className="config-section-surface">
          <div className="config-switch-row">
            <div>
              <div className="settings-row-label">启用自动压缩</div>
              <div className="settings-row-hint">为正常输入预留余量，并原样保留最近消息</div>
            </div>
            <Switch
              checked={draft.compaction.enabled ?? false}
              onCheckedChange={(enabled) => patch((next) => { next.compaction.enabled = enabled; })}
            />
          </div>
          <div className="settings-grid-2">
            <div className="settings-field">
              <Label className="settings-field-label">预留余量（tokens）</Label>
              <Input type="number" min={0} value={draft.compaction.reserve_tokens ?? ""} placeholder="16384" onChange={(event) => patch((next) => { next.compaction.reserve_tokens = event.target.value ? Number(event.target.value) : null; })} />
            </div>
            <div className="settings-field">
              <Label className="settings-field-label">保留最近消息（tokens）</Label>
              <Input type="number" min={0} value={draft.compaction.keep_recent_tokens ?? ""} placeholder="20000" onChange={(event) => patch((next) => { next.compaction.keep_recent_tokens = event.target.value ? Number(event.target.value) : null; })} />
            </div>
          </div>
        </div>
      </section>

      <section className="settings-section">
        <h3 className="settings-section-title">权限</h3>
        <p className="settings-section-desc">硬拒绝规则（设备路径、无法安全解析的路径）不受这里覆盖。</p>
        <div className="config-permission-list">
          {PERMISSION_FIELDS.map(({ key, label, hint }) => (
            <div className="config-permission-row" key={key}>
              <div className="config-permission-copy">
                <div className="settings-row-label">{label}</div>
                <div className="settings-row-hint">{hint}</div>
              </div>
              <select
                className={cn(SELECT_CLASS, "config-permission-select")}
                value={draft.permissions[key] ?? "allow"}
                onChange={(event) => patch((next) => { next.permissions[key] = event.target.value; })}
              >
                {RULE_OPTIONS.map((rule) => (
                  <option key={rule.value} value={rule.value}>{rule.label}</option>
                ))}
              </select>
            </div>
          ))}
        </div>
      </section>

      <div className="config-save-bar">
        <div className={cn("config-save-status", validationIssue && "is-error")}>
          {validationIssue ?? (dirty ? "配置有尚未保存的更改" : "配置已是最新状态")}
        </div>
        <Button onClick={() => void handleSave()} disabled={!dirty || saving || Boolean(validationIssue)}>
          {saved ? <Check size={14} /> : <Save size={14} />}
          {saved ? "已保存" : saving ? "保存中..." : "保存配置"}
        </Button>
      </div>
    </div>
  );
}

function ProvidersEditor({
  providers,
  activeProvider,
  onChange,
}: {
  providers: ProviderDto[];
  activeProvider: string;
  onChange: (providers: ProviderDto[], activeProvider: string) => void;
}) {
  const initialOpenIndex = Math.max(0, providers.findIndex((provider) => provider.name === activeProvider));
  const [openProviders, setOpenProviders] = useState<Set<number>>(
    () => new Set(providers.length > 0 ? [initialOpenIndex] : []),
  );
  const [openModels, setOpenModels] = useState<Set<string>>(() => new Set());

  useEffect(() => {
    const activeIndex = providers.findIndex((provider) => provider.name === activeProvider);
    if (activeIndex < 0) return;
    setOpenProviders((current) => {
      if (current.has(activeIndex)) return current;
      const next = new Set(current);
      next.add(activeIndex);
      return next;
    });
  }, [activeProvider, providers]);

  const commit = (nextProviders: ProviderDto[], nextActiveProvider = activeProvider) => {
    onChange(nextProviders, nextActiveProvider);
  };

  const updateProvider = (providerIndex: number, fn: (provider: ProviderDto) => void) => {
    const nextProviders = structuredClone(providers);
    const oldName = nextProviders[providerIndex].name;
    fn(nextProviders[providerIndex]);
    const nextName = nextProviders[providerIndex].name;
    commit(nextProviders, activeProvider === oldName ? nextName : activeProvider);
  };

  const addProvider = () => {
    const provider = emptyProvider();
    provider.name = nextAvailableName("provider", providers.map((item) => item.name));
    const nextIndex = providers.length;
    const hasValidActiveProvider = providers.some((item) => item.name === activeProvider);
    commit([...providers, provider], hasValidActiveProvider ? activeProvider : provider.name);
    setOpenProviders((current) => new Set(current).add(nextIndex));
  };

  const removeProvider = (providerIndex: number) => {
    const removedName = providers[providerIndex].name;
    const nextProviders = providers.filter((_, index) => index !== providerIndex);
    const activeProviderStillExists = nextProviders.some((provider) => provider.name === activeProvider);
    const nextActive = activeProvider !== removedName && activeProviderStillExists
      ? activeProvider
      : (nextProviders[Math.min(providerIndex, nextProviders.length - 1)]?.name ?? "");
    commit(nextProviders, nextActive);
    setOpenProviders((current) => {
      const next = new Set<number>();
      current.forEach((index) => {
        if (index < providerIndex) next.add(index);
        if (index > providerIndex) next.add(index - 1);
      });
      return next;
    });
    setOpenModels(new Set());
  };

  const toggleProvider = (providerIndex: number) => {
    setOpenProviders((current) => {
      const next = new Set(current);
      if (next.has(providerIndex)) next.delete(providerIndex);
      else next.add(providerIndex);
      return next;
    });
  };

  const updateModel = (
    providerIndex: number,
    modelIndex: number,
    fn: (model: ModelDto) => void,
  ) => {
    updateProvider(providerIndex, (provider) => {
      const oldName = provider.models[modelIndex].name;
      fn(provider.models[modelIndex]);
      if (provider.default_model === oldName) {
        provider.default_model = provider.models[modelIndex].name;
      }
    });
  };

  const addModel = (providerIndex: number) => {
    const nextModelIndex = providers[providerIndex].models.length;
    updateProvider(providerIndex, (provider) => {
      const model = emptyModel();
      model.name = nextAvailableName("model", provider.models.map((item) => item.name));
      provider.models.push(model);
      if (!provider.default_model) provider.default_model = model.name;
    });
    setOpenModels((current) => new Set(current).add(`${providerIndex}:${nextModelIndex}`));
  };

  const removeModel = (providerIndex: number, modelIndex: number) => {
    updateProvider(providerIndex, (provider) => {
      const removedName = provider.models[modelIndex].name;
      provider.models.splice(modelIndex, 1);
      if (provider.default_model === removedName) {
        provider.default_model = provider.models[0]?.name ?? null;
      }
    });
    setOpenModels((current) => {
      const next = new Set<string>();
      current.forEach((key) => {
        const [rawProviderIndex, rawModelIndex] = key.split(":").map(Number);
        if (rawProviderIndex !== providerIndex) next.add(key);
        else if (rawModelIndex < modelIndex) next.add(key);
        else if (rawModelIndex > modelIndex) next.add(`${providerIndex}:${rawModelIndex - 1}`);
      });
      return next;
    });
  };

  const toggleModel = (providerIndex: number, modelIndex: number) => {
    const key = `${providerIndex}:${modelIndex}`;
    setOpenModels((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  return (
    <div className="config-provider-editor">
      <div className="config-provider-list">
        {providers.length === 0 && (
          <div className="config-provider-empty">
            <Wrench size={18} />
            <div>
              <strong>尚未配置供应商</strong>
              <span>添加一个供应商后，即可配置连接和模型。</span>
            </div>
          </div>
        )}

        {providers.map((provider, providerIndex) => {
          const isOpen = openProviders.has(providerIndex);
          const isActive = activeProvider === provider.name;
          const modelSummary = provider.default_model || provider.models[0]?.name || "未选择默认模型";

          return (
            <div className={cn("config-provider", isOpen && "is-open")} key={providerIndex}>
              <div className="config-provider-header">
                <button
                  type="button"
                  className="config-provider-toggle"
                  aria-expanded={isOpen}
                  onClick={() => toggleProvider(providerIndex)}
                >
                  {isOpen ? <ChevronDown size={15} /> : <ChevronRight size={15} />}
                  <span className="config-provider-icon"><Wrench size={14} /></span>
                  <span className="config-provider-summary">
                    <span className="config-provider-title-line">
                      <strong>{provider.name || "未命名供应商"}</strong>
                      {isActive && <span className="config-provider-badge">当前使用</span>}
                    </span>
                    <span className="config-provider-meta">
                      {provider.api || "未设置 API"} · {modelSummary} · {provider.models.length} 个模型
                    </span>
                  </span>
                </button>
                <div className="config-provider-actions">
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    title="复制供应商配置"
                    aria-label={`复制 ${provider.name || "供应商"} 配置`}
                    onClick={() => void navigator.clipboard.writeText(JSON.stringify(provider, null, 2))}
                  >
                    <Copy size={13} />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    className="config-danger-button"
                    title="删除供应商"
                    aria-label={`删除 ${provider.name || "供应商"}`}
                    onClick={() => removeProvider(providerIndex)}
                  >
                    <Trash2 size={13} />
                  </Button>
                </div>
              </div>

              {isOpen && (
                <div className="config-provider-body">
                  <div className="config-subsection-heading">
                    <div>
                      <h4>连接</h4>
                      <p>API 类型、服务地址和鉴权信息。</p>
                    </div>
                    {!isActive && (
                      <Button variant="outline" size="sm" onClick={() => commit(providers, provider.name)}>
                        设为首选
                      </Button>
                    )}
                  </div>
                  <div className="settings-grid-2">
                    <div className="settings-field">
                      <Label className="settings-field-label">供应商名称</Label>
                      <Input value={provider.name} spellCheck={false} onChange={(event) => updateProvider(providerIndex, (next) => { next.name = event.target.value; })} />
                    </div>
                    <div className="settings-field">
                      <Label className="settings-field-label">API 类型</Label>
                      <Input value={provider.api} placeholder="messages / responses" spellCheck={false} onChange={(event) => updateProvider(providerIndex, (next) => { next.api = event.target.value; })} />
                    </div>
                    <div className="settings-field">
                      <Label className="settings-field-label">Profile</Label>
                      <Input value={provider.profile ?? ""} placeholder="anthropic / openai / deepseek-responses" spellCheck={false} onChange={(event) => updateProvider(providerIndex, (next) => { next.profile = event.target.value || null; })} />
                    </div>
                    <div className="settings-field">
                      <Label className="settings-field-label">Base URL</Label>
                      <Input value={provider.base_url} placeholder="https://api.example.com" spellCheck={false} onChange={(event) => updateProvider(providerIndex, (next) => { next.base_url = event.target.value; })} />
                    </div>
                    <div className="settings-field">
                      <Label className="settings-field-label">API Key 环境变量</Label>
                      <Input value={provider.api_key_env ?? ""} placeholder="OPENAI_API_KEY" spellCheck={false} onChange={(event) => updateProvider(providerIndex, (next) => { next.api_key_env = event.target.value || null; })} />
                    </div>
                    <div className="settings-field">
                      <Label className="settings-field-label">API Key（直接填写）</Label>
                      <Input type="password" value={provider.api_key ?? ""} autoComplete="off" spellCheck={false} placeholder="优先建议使用环境变量" onChange={(event) => updateProvider(providerIndex, (next) => { next.api_key = event.target.value || null; })} />
                    </div>
                  </div>

                  <div className="config-subsection-heading config-subsection-divider">
                    <div>
                      <h4>默认模型</h4>
                      <p>新会话默认使用此供应商下的哪个模型。</p>
                    </div>
                  </div>
                  <div className="config-default-model-field">
                    <div className="settings-field">
                      <Label className="settings-field-label">默认模型</Label>
                      <Input
                        list={`provider-${providerIndex}-models`}
                        value={provider.default_model ?? ""}
                        spellCheck={false}
                        onChange={(event) => updateProvider(providerIndex, (next) => { next.default_model = event.target.value || null; })}
                      />
                      <datalist id={`provider-${providerIndex}-models`}>
                        {provider.models.map((model, modelIndex) => <option key={modelIndex} value={model.name} />)}
                      </datalist>
                    </div>
                  </div>

                  <div className="config-model-section">
                    <div className="config-subsection-heading config-subsection-divider">
                      <div>
                        <h4>模型</h4>
                        <p>配置上下文、输出上限和支持的推理等级。</p>
                      </div>
                      <Button variant="outline" size="sm" onClick={() => addModel(providerIndex)}>
                        <Plus size={13} /> 添加模型
                      </Button>
                    </div>

                    <div className="config-model-list">
                      {provider.models.length === 0 && (
                        <div className="config-model-empty">此供应商还没有模型。</div>
                      )}
                      {provider.models.map((model, modelIndex) => {
                        const modelKey = `${providerIndex}:${modelIndex}`;
                        const isModelOpen = openModels.has(modelKey);
                        return (
                          <div className={cn("config-model", isModelOpen && "is-open")} key={modelIndex}>
                            <div className="config-model-header">
                              <button
                                type="button"
                                className="config-model-toggle"
                                aria-expanded={isModelOpen}
                                onClick={() => toggleModel(providerIndex, modelIndex)}
                              >
                                {isModelOpen ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                                <span className="config-model-summary">
                                  <strong>{model.name || "未命名模型"}</strong>
                                  <span>
                                    {model.context_window ? `${model.context_window.toLocaleString()} context` : "默认 context"}
                                    {model.max_tokens ? ` · ${model.max_tokens.toLocaleString()} max` : ""}
                                  </span>
                                </span>
                              </button>
                              <Button
                                variant="ghost"
                                size="icon-sm"
                                className="config-danger-button"
                                title="删除模型"
                                aria-label={`删除 ${model.name || "模型"}`}
                                onClick={() => removeModel(providerIndex, modelIndex)}
                              >
                                <Trash2 size={13} />
                              </Button>
                            </div>

                            {isModelOpen && (
                              <div className="config-model-body">
                                <div className="settings-grid-2">
                                  <div className="settings-field">
                                    <Label className="settings-field-label">模型名称</Label>
                                    <Input value={model.name} spellCheck={false} onChange={(event) => updateModel(providerIndex, modelIndex, (next) => { next.name = event.target.value; })} />
                                  </div>
                                  <div className="settings-field">
                                    <Label className="settings-field-label">默认 Effort</Label>
                                    <Input value={model.default_effort ?? ""} placeholder="medium" spellCheck={false} onChange={(event) => updateModel(providerIndex, modelIndex, (next) => { next.default_effort = event.target.value || null; })} />
                                  </div>
                                  <div className="settings-field">
                                    <Label className="settings-field-label">Context Window</Label>
                                    <Input type="number" min={0} value={model.context_window ?? ""} onChange={(event) => updateModel(providerIndex, modelIndex, (next) => { next.context_window = event.target.value ? Number(event.target.value) : null; })} />
                                  </div>
                                  <div className="settings-field">
                                    <Label className="settings-field-label">Max Tokens</Label>
                                    <Input type="number" min={0} value={model.max_tokens ?? ""} onChange={(event) => updateModel(providerIndex, modelIndex, (next) => { next.max_tokens = event.target.value ? Number(event.target.value) : null; })} />
                                  </div>
                                </div>
                                <div className="settings-field config-wide-field">
                                  <Label className="settings-field-label">Efforts（逗号分隔）</Label>
                                  <Input
                                    value={model.efforts.join(", ")}
                                    placeholder="low, medium, high"
                                    spellCheck={false}
                                    onChange={(event) => updateModel(providerIndex, modelIndex, (next) => {
                                      next.efforts = event.target.value.split(",").map((value) => value.trim()).filter(Boolean);
                                    })}
                                  />
                                  <span className="config-field-hint">留空时按 Profile 使用标准列表；不支持 effort 的模型可保存为空数组。</span>
                                </div>
                              </div>
                            )}
                          </div>
                        );
                      })}
                    </div>
                  </div>
                </div>
              )}
            </div>
          );
        })}
      </div>

      <Button variant="outline" size="sm" className="config-add-provider" onClick={addProvider}>
        <Plus size={13} /> 添加供应商
      </Button>
    </div>
  );
}

function RawConfig() {
  const configText = useStore((state) => state.configText);
  const loadConfig = useStore((state) => state.loadConfig);
  const saveConfig = useStore((state) => state.saveConfig);
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
          直接编辑 config.toml（保留全部注释）。保存后新会话生效。可视化编辑会重写部分结构并可能丢失注释，建议优先使用此模式保留自定义内容。
        </p>
        <Textarea
          className="mono h-[420px] resize-none text-[12.5px]"
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          spellCheck={false}
          style={{ lineHeight: 1.55 }}
        />
        <div className="mt-3 flex items-center gap-3">
          <Button
            onClick={() => {
              setSaving(true);
              void saveConfig(draft).finally(() => setSaving(false));
            }}
            disabled={!dirty || saving}
          >
            <Save size={14} /> {saving ? "保存中..." : "保存"}
          </Button>
          <Button variant="ghost" size="sm" onClick={() => setDraft(configText)}>
            放弃更改
          </Button>
          {!dirty && <span className="text-[11.5px] text-[var(--text-faint)]">暂无更改</span>}
        </div>
      </div>
      <div className="flex items-center gap-2 text-[11.5px] text-[var(--text-faint)]">
        <ChevronDown size={12} className="rotate-180" />
        配置文件路径：%APPDATA%/onemore/config.toml
      </div>
    </div>
  );
}
