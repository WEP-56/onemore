// 外观设置:主题模式、用户消息颜色、UI 缩放、字体。localStorage 持久化。

import { applyThemePreference, type ThemePreference } from "./theme";

export interface AppearanceSettings {
  theme: ThemePreference;
  userMessageColor: string;
  uiScale: number;
  uiFont: string;
  codeFont: string;
}

const KEY = "onemore-gui:appearance";

export const DEFAULT_APPEARANCE: AppearanceSettings = {
  theme: "system",
  userMessageColor: "",
  uiScale: 1,
  uiFont: "",
  codeFont: "",
};

const FONT_OPTIONS = [
  { label: "系统默认", value: "" },
  { label: "SF Pro / Segoe UI", value: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "Segoe UI", system-ui, sans-serif' },
  { label: "PingFang / 微软雅黑", value: '"PingFang SC", "Microsoft YaHei", system-ui, sans-serif' },
];

const CODE_FONT_OPTIONS = [
  { label: "系统默认", value: "" },
  { label: "SF Mono / Cascadia", value: '"SF Mono", "Cascadia Code", "Cascadia Mono", Consolas, monospace' },
  { label: "JetBrains Mono", value: '"JetBrains Mono", "SF Mono", Consolas, monospace' },
  { label: "Sarasa Mono SC", value: '"Sarasa Mono SC", "Noto Sans Mono CJK SC", Consolas, monospace' },
];

export function readAppearance(): AppearanceSettings {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as Partial<AppearanceSettings>;
      return {
        theme: parsed.theme === "light" || parsed.theme === "dark" || parsed.theme === "system" ? parsed.theme : DEFAULT_APPEARANCE.theme,
        userMessageColor: typeof parsed.userMessageColor === "string" ? parsed.userMessageColor : "",
        uiScale: typeof parsed.uiScale === "number" && parsed.uiScale >= 0.8 && parsed.uiScale <= 1.4 ? parsed.uiScale : 1,
        uiFont: typeof parsed.uiFont === "string" ? parsed.uiFont : "",
        codeFont: typeof parsed.codeFont === "string" ? parsed.codeFont : "",
      };
    }
  } catch {
    // ignore
  }
  return { ...DEFAULT_APPEARANCE };
}

export function writeAppearance(settings: AppearanceSettings): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(settings));
  } catch {
    // ignore
  }
  applyAppearance(settings);
}

export function applyAppearance(settings: AppearanceSettings): void {
  applyThemePreference(settings.theme);
  const root = document.documentElement;
  root.style.setProperty("--ui-scale", String(settings.uiScale));
  root.style.fontSize = `${14 * settings.uiScale}px`;
  if (settings.userMessageColor) {
    root.style.setProperty("--surface-bubble-user", settings.userMessageColor);
    root.style.setProperty("--color-message-user-bg", settings.userMessageColor);
  } else {
    root.style.removeProperty("--surface-bubble-user");
    root.style.removeProperty("--color-message-user-bg");
  }
  if (settings.uiFont) root.style.setProperty("--ui-font-family", settings.uiFont);
  else root.style.removeProperty("--ui-font-family");
  if (settings.codeFont) root.style.setProperty("--code-font-family", settings.codeFont);
  else root.style.removeProperty("--code-font-family");
}

export function initAppearance(): void {
  applyAppearance(readAppearance());
}

export { FONT_OPTIONS, CODE_FONT_OPTIONS };
