// 侧栏折叠状态,localStorage 持久化。

import { useCallback, useEffect, useState } from "react";

const SIDEBAR_COLLAPSED_KEY = "onemore-gui:sidebar-collapsed";

function readCollapsed(): boolean {
  try {
    return localStorage.getItem(SIDEBAR_COLLAPSED_KEY) === "1";
  } catch {
    return false;
  }
}

export function useSidebarToggles() {
  const [sidebarCollapsed, setSidebarCollapsed] = useState(readCollapsed);

  useEffect(() => {
    try {
      localStorage.setItem(SIDEBAR_COLLAPSED_KEY, sidebarCollapsed ? "1" : "0");
    } catch {
      // ignore
    }
    const app = document.querySelector<HTMLElement>(".app");
    app?.classList.toggle("sidebar-collapsed", sidebarCollapsed);
  }, [sidebarCollapsed]);

  const collapseSidebar = useCallback(() => setSidebarCollapsed(true), []);
  const expandSidebar = useCallback(() => setSidebarCollapsed(false), []);
  const toggleSidebar = useCallback(() => setSidebarCollapsed((v) => !v), []);

  return { sidebarCollapsed, collapseSidebar, expandSidebar, toggleSidebar };
}
