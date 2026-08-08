// 基础设置:仅外观调整。主题(默认/跟随系统/浅色/深色)、用户消息颜色、UI 缩放、字体。

import { useCallback, useState } from "react";
import { RotateCcw } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  CODE_FONT_OPTIONS,
  DEFAULT_APPEARANCE,
  FONT_OPTIONS,
  readAppearance,
  writeAppearance,
  type AppearanceSettings,
} from "@/lib/appearance";
import { cn } from "@/lib/utils";

const THEME_MODES = [
  { id: "system", label: "跟随系统" },
  { id: "light", label: "浅色" },
  { id: "dark", label: "深色" },
] as const;

export default function AppearanceSection() {
  const [settings, setSettings] = useState<AppearanceSettings>(() => readAppearance());

  const update = useCallback((patch: Partial<AppearanceSettings>) => {
    setSettings((prev) => {
      const next = { ...prev, ...patch };
      writeAppearance(next);
      return next;
    });
  }, []);

  const reset = () => {
    setSettings(DEFAULT_APPEARANCE);
    writeAppearance({ ...DEFAULT_APPEARANCE });
  };

  return (
    <div>
      <div className="settings-section">
        <h3 className="settings-section-title">主题</h3>
        <p className="settings-section-desc">默认主题为跟随系统;也可手动指定浅色或深色外观。</p>
        <div className="settings-row">
          <div>
            <div className="settings-row-label">外观模式</div>
            <div className="settings-row-hint">默认跟随操作系统设置</div>
          </div>
          <div className="settings-seg">
            {THEME_MODES.map((m) => (
              <button
                key={m.id}
                type="button"
                className={cn(settings.theme === m.id && "is-active")}
                onClick={() => update({ theme: m.id })}
              >
                {m.label}
              </button>
            ))}
          </div>
        </div>
        <div className="settings-row">
          <div>
            <div className="settings-row-label">用户消息颜色</div>
            <div className="settings-row-hint">对话中用户气泡的背景色</div>
          </div>
          <div className="flex items-center gap-2">
            <input
              type="color"
              value={settings.userMessageColor || (settings.theme === "light" ? "#0078d4" : "#005fb8")}
              onChange={(e) => update({ userMessageColor: e.target.value })}
              style={{ width: 34, height: 26, padding: 0, border: "1px solid var(--border-strong)", borderRadius: 6, background: "transparent", cursor: "pointer" }}
            />
            <Button variant="ghost" size="icon-sm" title="恢复默认" onClick={() => update({ userMessageColor: "" })}>
              <RotateCcw size={13} />
            </Button>
          </div>
        </div>
      </div>

      <div className="settings-section">
        <h3 className="settings-section-title">显示</h3>
        <div className="settings-row">
          <div>
            <div className="settings-row-label">界面缩放</div>
            <div className="settings-row-hint">{Math.round(settings.uiScale * 100)}%</div>
          </div>
          <input
            type="range"
            min={0.8}
            max={1.4}
            step={0.05}
            value={settings.uiScale}
            onChange={(e) => update({ uiScale: Number(e.target.value) })}
            style={{ width: 180, accentColor: "var(--status-success)" }}
          />
        </div>
        <div className="settings-row">
          <div className="settings-row-label">界面字体</div>
          <select
            className="h-8 rounded-md border px-2 text-[12.5px] outline-none"
            style={{ borderColor: "var(--border-strong)", background: "var(--surface-control)", color: "var(--text-primary)" }}
            value={settings.uiFont}
            onChange={(e) => update({ uiFont: e.target.value })}
          >
            {FONT_OPTIONS.map((f) => (
              <option key={f.label} value={f.value}>{f.label}</option>
            ))}
          </select>
        </div>
        <div className="settings-row">
          <div className="settings-row-label">代码字体</div>
          <select
            className="h-8 rounded-md border px-2 text-[12.5px] outline-none"
            style={{ borderColor: "var(--border-strong)", background: "var(--surface-control)", color: "var(--text-primary)" }}
            value={settings.codeFont}
            onChange={(e) => update({ codeFont: e.target.value })}
          >
            {CODE_FONT_OPTIONS.map((f) => (
              <option key={f.label} value={f.value}>{f.label}</option>
            ))}
          </select>
        </div>
        <div className="settings-row">
          <div>
            <div className="settings-row-label">恢复默认外观</div>
            <div className="settings-row-hint">重置为系统默认字体、缩放与颜色</div>
          </div>
          <Button variant="outline" size="sm" onClick={reset}>
            <RotateCcw size={13} /> 重置
          </Button>
        </div>
      </div>
    </div>
  );
}
