import { useState } from "react";
import { copyText, shortId } from "../app/util";

/// 显示缩短后的 ID，点击复制完整值。
export default function CopyId({ value, len = 10 }: { value: string; len?: number }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      className="copy-id mono"
      title={`复制完整值：${value}`}
      onClick={() => {
        void copyText(value).then((ok) => {
          if (ok) {
            setCopied(true);
            setTimeout(() => setCopied(false), 1200);
          }
        });
      }}
    >
      {copied ? "已复制" : shortId(value, len)}
    </button>
  );
}
