import type { ComponentProps } from "react";
import { Streamdown } from "streamdown";
import "streamdown/styles.css";

interface AgentMarkdownProps {
  children: string;
  streaming: boolean;
  onOpenExternalUrl(url: string): void;
}

function safeWebUrl(value: string): string | null {
  try {
    const url = new URL(value);
    return ["http:", "https:"].includes(url.protocol) && url.host
      ? url.href
      : null;
  } catch {
    return null;
  }
}

export function AgentMarkdown({
  children,
  streaming,
  onOpenExternalUrl,
}: AgentMarkdownProps) {
  const Link = ({
    href,
    children: label,
  }: ComponentProps<"a"> & { node?: unknown }) => {
    const safeUrl = href ? safeWebUrl(href) : null;
    if (!safeUrl) return <span>{label}</span>;
    return (
      <button
        className="agent-markdown-link"
        type="button"
        onClick={() => onOpenExternalUrl(safeUrl)}
      >
        {label}
      </button>
    );
  };

  return (
    <Streamdown
      className="agent-markdown"
      mode={streaming ? "streaming" : "static"}
      parseIncompleteMarkdown
      isAnimating={streaming}
      animated={false}
      controls={false}
      skipHtml
      urlTransform={(url) => safeWebUrl(url)}
      components={{
        a: Link,
        // 远程图片不进入对话 WebView；引用仍可通过受控文本链接打开。
        img: () => null,
      }}
    >
      {children}
    </Streamdown>
  );
}
