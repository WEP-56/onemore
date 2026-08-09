import { useEffect, useMemo, useState } from "react";
import { Copy, Minus, Square, X } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { isTauri } from "@tauri-apps/api/core";

export default function WindowControls() {
  const [maximized, setMaximized] = useState(false);
  const appWindow = useMemo(() => (isTauri() ? getCurrentWindow() : null), []);

  useEffect(() => {
    if (!appWindow) return;
    let active = true;
    const sync = () => {
      void appWindow.isMaximized().then((value) => active && setMaximized(value));
    };
    sync();
    const unlisten = appWindow.onResized(sync);
    return () => {
      active = false;
      void unlisten.then((dispose) => dispose());
    };
  }, [appWindow]);

  return (
    <div className="window-controls" data-tauri-drag-region="false">
      <button type="button" className="window-control-button" aria-label="最小化" title="最小化" onClick={() => appWindow && void appWindow.minimize()}>
        <Minus size={14} />
      </button>
      <button
        type="button"
        className="window-control-button"
        aria-label={maximized ? "还原" : "最大化"}
        title={maximized ? "还原" : "最大化"}
        onClick={() => appWindow && void appWindow.toggleMaximize()}
      >
        {maximized ? <Copy size={11} /> : <Square size={11} />}
      </button>
      <button type="button" className="window-control-button window-control-button--close" aria-label="关闭" title="关闭" onClick={() => appWindow && void appWindow.close()}>
        <X size={15} />
      </button>
    </div>
  );
}
