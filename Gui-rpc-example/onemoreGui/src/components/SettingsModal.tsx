import { useEffect, useState } from "react";
import { useStore } from "@/app/store";
import { X, Save } from "lucide-react";

export default function SettingsModal() {
  const open = useStore((s) => s.settingsOpen);
  const setOpen = useStore((s) => s.setSettingsOpen);
  const loadConfig = useStore((s) => s.loadConfig);
  const saveConfig = useStore((s) => s.saveConfig);
  const configText = useStore((s) => s.configText);
  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (open) {
      void loadConfig();
      setDraft(configText);
    }
  }, [open]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (open) setDraft(configText);
  }, [configText, open]);

  if (!open) return null;

  const handleSave = async () => {
    setSaving(true);
    await saveConfig(draft);
    setSaving(false);
    setOpen(false);
  };

  return (
    <div
      className="fixed inset-0 z-[100] flex items-center justify-center"
      style={{ background: "rgba(0,0,0,0.5)" }}
      onClick={() => setOpen(false)}
    >
      <div
        className="flex h-[70vh] max-h-[600px] w-[640px] max-w-[calc(100vw-48px)] flex-col gap-3 rounded-lg p-6"
        style={{ background: "var(--surface-card)", border: "1px solid var(--border-strong)", boxShadow: "0 8px 24px rgba(0,0,0,0.4)" }}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between">
          <h2 className="m-0 text-base font-semibold">设置 — config.toml</h2>
          <button type="button" className="flex h-7 w-7 items-center justify-center rounded transition-colors hover:bg-[var(--surface-hover)]" onClick={() => setOpen(false)}>
            <X size={16} className="text-[var(--text-muted)]" />
          </button>
        </div>
        <p className="m-0 text-[13px] text-[var(--text-faint)]">
          编辑 Onemore 配置文件。保存后新会话生效，运行中的会话不受影响。
        </p>
        <textarea
          className="mono flex-1 resize-none rounded-md p-3 text-[13px] outline-none"
          style={{ background: "var(--surface-messages)", border: "1px solid var(--border-strong)", lineHeight: 1.5 }}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onFocus={(e) => (e.currentTarget.style.borderColor = "var(--status-success)")}
          onBlur={(e) => (e.currentTarget.style.borderColor = "var(--border-strong)")}
          spellCheck={false}
        />
        <div className="flex justify-end gap-2">
          <button
            type="button"
            className="rounded-md px-3.5 py-1.5 text-[13px] transition-colors hover:bg-[var(--surface-hover)]"
            style={{ border: "1px solid var(--border-strong)", background: "var(--surface-control)", color: "var(--text-primary)" }}
            onClick={() => setOpen(false)}
          >
            取消
          </button>
          <button
            type="button"
            className="flex items-center gap-1.5 rounded-md px-3.5 py-1.5 text-[13px] font-semibold text-black disabled:opacity-40"
            style={{ background: "var(--primary)" }}
            disabled={saving}
            onClick={() => void handleSave()}
          >
            <Save size={14} /> 保存
          </button>
        </div>
      </div>
    </div>
  );
}
