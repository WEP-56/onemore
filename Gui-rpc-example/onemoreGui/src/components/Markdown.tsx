// Markdown 渲染:react-markdown + remark-gfm + 代码块(带复制)。
// 视觉参照 desktop-cc-gui messages.part2.css。

import { memo, useCallback, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import remarkBreaks from "remark-breaks";
import { Check, Copy, FileText } from "lucide-react";
import { cn } from "@/lib/utils";

interface MarkdownProps {
  value: string;
  className?: string;
}

export const Markdown = memo(function Markdown({ value, className }: MarkdownProps) {
  return (
    <div className={cn("markdown", className)}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkBreaks]}
        components={{
          a: ({ href, children }) => (
            <a
              href={href}
              target="_blank"
              rel="noreferrer"
              onClick={(e) => {
                // 文件路径链接不交给浏览器
                if (href && /^[a-zA-Z]:[\\/]|^\/[^/]/.test(href)) {
                  e.preventDefault();
                }
              }}
            >
              {children}
            </a>
          ),
          pre: CodeBlock,
        }}
      >
        {value}
      </ReactMarkdown>
    </div>
  );
});

/** 代码块:语言标签 + 复制按钮。cc-gui 的 markdown-codeblock 视觉。 */
function CodeBlock({ children, ...props }: React.ComponentProps<"pre">) {
  const [copied, setCopied] = useState(false);
  const codeEl = extractCode(children);
  const lang = extractLang(codeEl.props?.className);

  const handleCopy = useCallback(() => {
    const text = String(codeEl.props?.children ?? "");
    void navigator.clipboard.writeText(text).then(() => {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    });
  }, [codeEl]);

  return (
    <div className="markdown-codeblock">
      <div className="markdown-codeblock-header">
        <span className="markdown-codeblock-language">
          <FileText size={13} className="markdown-codeblock-language-icon" />
          <span className="markdown-codeblock-language-text">{lang || "text"}</span>
        </span>
        <span className="markdown-codeblock-actions">
          <button
            type="button"
            className={cn("markdown-codeblock-copy", copied && "is-copied")}
            title="复制代码"
            onClick={handleCopy}
          >
            {copied ? <Check size={14} className="markdown-codeblock-copy-icon" /> : <Copy size={14} className="markdown-codeblock-copy-icon" />}
          </button>
        </span>
      </div>
      <pre {...props} className={cn(props.className, "markdown-codeblock-pre")}>
        {children}
      </pre>
    </div>
  );
}

function extractCode(node: React.ReactNode): { props: { className?: string; children?: React.ReactNode } } {
  if (node && typeof node === "object" && "props" in (node as object)) {
    const maybe = (node as { props?: { className?: string; children?: React.ReactNode } }).props;
    if (maybe && typeof maybe === "object") {
      return { props: maybe };
    }
  }
  return { props: {} };
}

function extractLang(className?: string): string | null {
  if (!className) return null;
  const m = /language-([\w+-]+)/.exec(className);
  return m ? m[1] : null;
}
