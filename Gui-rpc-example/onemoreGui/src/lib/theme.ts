// 主题偏好管理:跟随系统(默认)/浅色/深色。
// data-theme 缺省 = 跟随系统;显式 "light"/"dark" 覆盖系统偏好。

export type ThemePreference = "system" | "light" | "dark";

const THEME_STORAGE_KEY = "onemore-gui:theme-preference";

export function readThemePreference(): ThemePreference {
  try {
    const raw = localStorage.getItem(THEME_STORAGE_KEY);
    if (raw === "light" || raw === "dark" || raw === "system") return raw;
  } catch {
    // ignore
  }
  return "system";
}

export function writeThemePreference(pref: ThemePreference): void {
  try {
    localStorage.setItem(THEME_STORAGE_KEY, pref);
  } catch {
    // ignore
  }
  applyThemePreference(pref);
}

export function applyThemePreference(pref: ThemePreference): void {
  const root = document.documentElement;
  if (pref === "system") {
    root.removeAttribute("data-theme");
  } else {
    root.setAttribute("data-theme", pref);
  }
}

export function getSystemAppearance(): "light" | "dark" {
  if (typeof window === "undefined" || !window.matchMedia) return "dark";
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export function watchSystemAppearance(cb: (appearance: "light" | "dark") => void): () => void {
  if (typeof window === "undefined" || !window.matchMedia) return () => {};
  const mq = window.matchMedia("(prefers-color-scheme: dark)");
  const handler = (e: MediaQueryListEvent) => cb(e.matches ? "dark" : "light");
  mq.addEventListener("change", handler);
  return () => mq.removeEventListener("change", handler);
}

/** 启动时在渲染前调用,避免主题闪烁。 */
export function initTheme(): void {
  applyThemePreference(readThemePreference());
}
