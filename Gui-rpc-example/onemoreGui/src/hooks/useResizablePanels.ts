// 可拖拽分栏:sidebar / right-panel 宽度,localStorage 持久化。
// 参考 desktop-cc-gui useResizablePanels 的简化版(无 layout-swapped)。

import type { MouseEvent as ReactMouseEvent } from "react";
import { useCallback, useEffect, useRef, useState } from "react";

const MIN_SIDEBAR_WIDTH = 210;
const MAX_SIDEBAR_WIDTH = 360;
const DEFAULT_SIDEBAR_WIDTH = 240;
const MIN_RIGHT_PANEL_WIDTH = 240;
const DEFAULT_RIGHT_PANEL_WIDTH = 300;

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

function getRightPanelMaxWidth() {
  if (typeof window === "undefined") return 420;
  return Math.max(420, Math.floor(window.innerWidth * 0.5));
}

function readStoredNum(key: string, fallback: number, min: number, max: number) {
  try {
    const stored = Number(localStorage.getItem(key));
    if (Number.isFinite(stored)) return clamp(stored, min, max);
  } catch {
    // ignore
  }
  return clamp(fallback, min, max);
}

function writeStoredNum(key: string, value: number) {
  try {
    localStorage.setItem(key, String(value));
  } catch {
    // ignore
  }
}

function getAppElement(): HTMLElement | null {
  return document.querySelector<HTMLElement>(".app");
}

function applyLiveSizeCssVar(name: string, value: number) {
  const app = getAppElement();
  if (!app) return;
  app.style.setProperty(name, `${value}px`);
}

function setPanelResizing(active: boolean) {
  if (active) {
    document.body.dataset.panelResizing = "true";
  } else {
    delete document.body.dataset.panelResizing;
  }
}

export function useResizablePanels() {
  const [sidebarWidth, setSidebarWidth] = useState(() =>
    readStoredNum("onemore-gui:sidebar-width", DEFAULT_SIDEBAR_WIDTH, MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH),
  );
  const [rightPanelWidth, setRightPanelWidthState] = useState(() =>
    readStoredNum("onemore-gui:right-panel-width", DEFAULT_RIGHT_PANEL_WIDTH, MIN_RIGHT_PANEL_WIDTH, getRightPanelMaxWidth()),
  );
  const resizeRef = useRef<{ type: "sidebar" | "right-panel"; startX: number; startWidth: number } | null>(null);

  useEffect(() => {
    writeStoredNum("onemore-gui:sidebar-width", sidebarWidth);
    applyLiveSizeCssVar("--sidebar-width", sidebarWidth);
  }, [sidebarWidth]);

  useEffect(() => {
    writeStoredNum("onemore-gui:right-panel-width", rightPanelWidth);
    applyLiveSizeCssVar("--right-panel-width", rightPanelWidth);
  }, [rightPanelWidth]);

  useEffect(() => {
    function syncRightPanelWidthToViewport() {
      setRightPanelWidthState((current) => clamp(current, MIN_RIGHT_PANEL_WIDTH, getRightPanelMaxWidth()));
    }
    window.addEventListener("resize", syncRightPanelWidthToViewport);
    return () => window.removeEventListener("resize", syncRightPanelWidthToViewport);
  }, []);

  useEffect(() => {
    function handleMouseMove(event: MouseEvent) {
      const active = resizeRef.current;
      if (!active) return;
      const delta = event.clientX - active.startX;
      if (active.type === "sidebar") {
        const next = clamp(active.startWidth + delta, MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
        applyLiveSizeCssVar("--sidebar-width", next);
      } else {
        const next = clamp(active.startWidth - delta, MIN_RIGHT_PANEL_WIDTH, getRightPanelMaxWidth());
        applyLiveSizeCssVar("--right-panel-width", next);
      }
    }
    function handleMouseUp() {
      if (!resizeRef.current) return;
      const type = resizeRef.current.type;
      resizeRef.current = null;
      setPanelResizing(false);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      document.body.style.webkitUserSelect = "";
      // 从 css var 读回最终值并落 state
      const app = getAppElement();
      if (app) {
        const raw = app.style.getPropertyValue(type === "sidebar" ? "--sidebar-width" : "--right-panel-width");
        const parsed = Number.parseFloat(raw);
        if (Number.isFinite(parsed)) {
          if (type === "sidebar") setSidebarWidth(parsed);
          else setRightPanelWidthState(parsed);
        }
      }
    }
    window.addEventListener("mousemove", handleMouseMove);
    window.addEventListener("mouseup", handleMouseUp);
    window.addEventListener("blur", handleMouseUp);
    return () => {
      window.removeEventListener("mousemove", handleMouseMove);
      window.removeEventListener("mouseup", handleMouseUp);
      window.removeEventListener("blur", handleMouseUp);
    };
  }, []);

  const startResize = useCallback(
    (type: "sidebar" | "right-panel") =>
      (event: ReactMouseEvent) => {
        if (event.button !== 0) return;
        event.preventDefault();
        event.stopPropagation();
        resizeRef.current = {
          type,
          startX: event.clientX,
          startWidth: type === "sidebar" ? sidebarWidth : rightPanelWidth,
        };
        setPanelResizing(true);
        document.body.style.cursor = "col-resize";
        document.body.style.userSelect = "none";
        document.body.style.webkitUserSelect = "none";
      },
    [rightPanelWidth, sidebarWidth],
  );

  return {
    sidebarWidth,
    rightPanelWidth,
    onSidebarResizeStart: startResize("sidebar"),
    onRightPanelResizeStart: startResize("right-panel"),
  };
}
