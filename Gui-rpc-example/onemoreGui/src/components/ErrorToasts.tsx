// 全局提示:错误 toast(右下角,自动消失)。参照 cc-gui error-toasts 交互。

import { useEffect, useRef, useState } from "react";
import { useStore } from "@/app/store";
import { AlertTriangle, X } from "lucide-react";

export default function ErrorToasts() {
  const lastError = useStore((s) => s.lastError);
  const [visible, setVisible] = useState<{ code: string; message: string; at: number } | null>(null);
  const timerRef = useRef<number | null>(null);
  const lastKeyRef = useRef<string>("");

  useEffect(() => {
    if (!lastError) return;
    const key = `${lastError.code}:${lastError.message}`;
    if (key === lastKeyRef.current) return;
    lastKeyRef.current = key;
    setVisible({ ...lastError, at: Date.now() });

    if (timerRef.current != null) window.clearTimeout(timerRef.current);
    timerRef.current = window.setTimeout(() => {
      setVisible(null);
      timerRef.current = null;
    }, 6000);
  }, [lastError]);

  useEffect(() => {
    return () => {
      if (timerRef.current != null) window.clearTimeout(timerRef.current);
    };
  }, []);

  if (!visible) return null;

  return (
    <div className="error-toast-host">
      <div className="error-toast">
        <AlertTriangle size={15} className="error-toast-icon" />
        <div className="error-toast-body">
          <div className="error-toast-code mono">{visible.code}</div>
          <div className="error-toast-message">{visible.message}</div>
        </div>
        <button
          type="button"
          className="flex h-6 w-6 shrink-0 items-center justify-center rounded text-[var(--text-faint)] transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--text-strong)]"
          onClick={() => setVisible(null)}
        >
          <X size={13} />
        </button>
      </div>
    </div>
  );
}
