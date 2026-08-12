import { useRef, useState, type FormEvent } from "react";

import type {
  AgentConversationMessage,
  AgentPageContext,
  AgentSearchResult,
  AgentStreamEvent,
  AiPreferences,
} from "../domain";
import agentMark from "../assets/ui/skillyard-agent-sprout-mark.png";
import { useI18n } from "../i18n";
import { AgentMarkdown } from "./AgentMarkdown";

interface AgentOverlayProps {
  context: AgentPageContext;
  aiPreferences: AiPreferences;
  onAsk(
    requestId: string,
    context: AgentPageContext,
    messages: AgentConversationMessage[],
    onEvent: (event: AgentStreamEvent) => void,
  ): Promise<void>;
  onCancel(requestId: string): Promise<void>;
  onOpenExternalUrl(url: string): Promise<void>;
  onPreviewInstall(result: AgentSearchResult): Promise<void>;
}

interface AgentUiMessage extends AgentConversationMessage {
  id: string;
  status: "complete" | "streaming" | "incomplete";
  searchResults?: AgentSearchResult[];
}

export function AgentOverlay({
  context,
  aiPreferences,
  onAsk,
  onCancel,
  onOpenExternalUrl,
  onPreviewInstall,
}: AgentOverlayProps) {
  const { t } = useI18n();
  const [isOpen, setIsOpen] = useState(false);
  const [messages, setMessages] = useState<AgentUiMessage[]>([]);
  const [draft, setDraft] = useState("");
  const [isSending, setIsSending] = useState(false);
  const [previewingUrl, setPreviewingUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const sessionGeneration = useRef(0);
  const sequence = useRef(0);
  const activeRequestId = useRef<string | null>(null);
  const isReady =
    aiPreferences.enabled &&
    aiPreferences.disclosureAccepted &&
    aiPreferences.hasApiKey &&
    aiPreferences.verified;

  const closeDrawer = () => {
    // 关闭只收起抽屉，返回后仍能继续当前只读会话。
    setIsOpen(false);
  };

  const endSession = () => {
    // 只有“结束会话”会取消请求并清空当前 Session。
    const requestId = activeRequestId.current;
    if (requestId) {
      void onCancel(requestId).catch(() => {
        // Session 已在本地销毁；取消失败不能让旧回答重新进入新 Session。
      });
    }
    activeRequestId.current = null;
    sessionGeneration.current += 1;
    setMessages([]);
    setDraft("");
    setError(null);
    setIsSending(false);
    setPreviewingUrl(null);
  };

  const send = async (event: FormEvent) => {
    event.preventDefault();
    const question = draft.trim();
    if (!question || isSending || !isReady) return;
    const requestId = `agent-${sessionGeneration.current}-${++sequence.current}`;
    const userMessage: AgentUiMessage = {
      id: `${requestId}-user`,
      role: "user",
      content: question,
      status: "complete",
    };
    const assistantMessage: AgentUiMessage = {
      id: `${requestId}-assistant`,
      role: "assistant",
      content: "",
      status: "streaming",
    };
    const conversation = [
      ...messages
        .filter(
          (message) =>
            message.role === "user" || message.status === "complete",
        )
        .map(({ role, content }) => ({ role, content })),
      { role: "user", content: question } satisfies AgentConversationMessage,
    ];
    setMessages((current) => [
      ...current,
      userMessage,
      assistantMessage,
    ]);
    setDraft("");
    setError(null);
    setIsSending(true);
    activeRequestId.current = requestId;
    const activeGeneration = sessionGeneration.current;
    let terminalReceived = false;
    try {
      await onAsk(requestId, context, conversation, (streamEvent) => {
        if (
          sessionGeneration.current !== activeGeneration ||
          activeRequestId.current !== requestId
        ) {
          return;
        }
        if (streamEvent.type === "delta") {
          setMessages((current) =>
            current.map((message) =>
              message.id === assistantMessage.id
                ? { ...message, content: message.content + streamEvent.text }
                : message,
            ),
          );
          return;
        }
        terminalReceived = true;
        activeRequestId.current = null;
        setIsSending(false);
        if (streamEvent.type === "completed") {
          setMessages((current) =>
            current.map((message) =>
              message.id === assistantMessage.id
                ? {
                    ...message,
                    status: "complete",
                    searchResults: streamEvent.searchResults,
                  }
                : message,
            ),
          );
          return;
        }
        setMessages((current) =>
          current.map((message) =>
            message.id === assistantMessage.id
              ? { ...message, status: "incomplete" }
              : message,
          ),
        );
        setError(streamEvent.message);
      });
      if (
        !terminalReceived &&
        sessionGeneration.current === activeGeneration &&
        activeRequestId.current === requestId
      ) {
        throw new Error(t("这次回答没有完成，请稍后重试。"));
      }
    } catch (cause) {
      if (sessionGeneration.current !== activeGeneration) return;
      activeRequestId.current = null;
      setMessages((current) =>
        current.map((message) =>
          message.id === assistantMessage.id
            ? { ...message, status: "incomplete" }
            : message,
        ),
      );
      setError(
        cause instanceof Error
          ? cause.message
          : typeof cause === "object" &&
              cause !== null &&
              "message" in cause &&
              typeof cause.message === "string"
            ? cause.message
            : t("这次回答没有完成，请稍后重试。"),
      );
    } finally {
      if (sessionGeneration.current === activeGeneration) {
        if (activeRequestId.current === requestId) {
          activeRequestId.current = null;
        }
        setIsSending(false);
      }
    }
  };

  const previewInstall = async (result: AgentSearchResult) => {
    if (previewingUrl) return;
    setPreviewingUrl(result.url);
    setError(null);
    try {
      await onPreviewInstall(result);
    } catch (cause) {
      setError(
        cause instanceof Error
          ? cause.message
          : t("无法准备这个来源的安装预览，请稍后重试。"),
      );
    } finally {
      setPreviewingUrl(null);
    }
  };

  return (
    <aside className="agent-overlay">
      {isOpen ? (
        <section
          className="agent-window"
          role="dialog"
          aria-label={t("SkillYard 助手")}
        >
          <header className="agent-window-header">
            <div className="agent-window-title">
              <p className="section-eyebrow">{t("只读解释与搜索")}</p>
              <h2>SkillYard Agent</h2>
            </div>
            <div className="agent-window-actions">
              <button
                className="agent-session-action"
                type="button"
                onClick={endSession}
              >
                {t("结束会话")}
              </button>
              <button
                className="agent-close"
                type="button"
                aria-label={t("关闭 SkillYard Agent")}
                onClick={closeDrawer}
              >
                {t("关闭")}
              </button>
            </div>
          </header>

          <div className="agent-messages" aria-live="polite">
            {messages.length === 0 ? (
              <div className="agent-empty">
                <strong>{t("可以从当前页面开始提问")}</strong>
                <p>
                  {t(
                    "助手只会读取 SkillYard 已知的内容并回答，不会执行安装、更新或删除。",
                  )}
                </p>
              </div>
            ) : (
              messages.map((message) => (
                <div
                  className={`agent-message ${message.role}`}
                  key={message.id}
                >
                  <span>
                    {message.role === "user" ? t("你") : t("SkillYard")}
                  </span>
                  {message.role === "assistant" ? (
                    message.content ? (
                      <AgentMarkdown
                        streaming={message.status === "streaming"}
                        onOpenExternalUrl={(url) => {
                          void onOpenExternalUrl(url);
                        }}
                      >
                        {message.content}
                      </AgentMarkdown>
                    ) : null
                  ) : (
                    <p>{message.content}</p>
                  )}
                  {message.status === "incomplete" ? (
                    <small className="agent-incomplete">
                      {t("回答未完成")}
                    </small>
                  ) : null}
                  {message.status === "complete" &&
                  message.searchResults &&
                  message.searchResults.length > 0 ? (
                    <ul className="agent-search-results">
                      {message.searchResults.map((result) => (
                        <li key={result.url}>
                          <div>
                            <button
                              className="agent-result-link"
                              type="button"
                              onClick={() => {
                                void onOpenExternalUrl(result.url);
                              }}
                            >
                              {result.title}
                            </button>
                            <small>
                              {result.kind === "reference"
                                ? t("参考链接")
                                : t("可准备安装预览")}
                            </small>
                          </div>
                          {result.kind !== "reference" ? (
                            <button
                              type="button"
                              aria-label={t("查看 {title} 的安装预览", {
                                title: result.title,
                              })}
                              disabled={previewingUrl !== null}
                              onClick={() => previewInstall(result)}
                            >
                              {previewingUrl === result.url
                                ? t("正在准备…")
                                : t("查看安装预览")}
                            </button>
                          ) : null}
                        </li>
                      ))}
                    </ul>
                  ) : null}
                </div>
              ))
            )}
            {isSending ? (
              <p className="agent-thinking">{t("正在阅读并回答…")}</p>
            ) : null}
          </div>

          {!isReady ? (
            <div className="agent-configuration-note" role="status">
              {t("请先在设置中启用 AI、保存 API Key 并完成连接测试。")}
            </div>
          ) : null}
          {error ? (
            <div className="inline-error" role="alert">
              {error}
            </div>
          ) : null}

          <form className="agent-composer" onSubmit={send}>
            <label>
              <span className="visually-hidden">{t("向 SkillYard 提问")}</span>
              <input
                aria-label={t("向 SkillYard 提问")}
                value={draft}
                disabled={!isReady || isSending}
                placeholder={t("搜索 Bundle，或询问挂载状态")}
                onChange={(event) => setDraft(event.target.value)}
              />
            </label>
            <button
              type="submit"
              disabled={!isReady || isSending || draft.trim().length === 0}
            >
              {t("发送")}
            </button>
          </form>
        </section>
      ) : null}

      <button
        className="agent-launcher"
        type="button"
        aria-label={t("打开 SkillYard 助手")}
        aria-expanded={isOpen}
        onClick={() => setIsOpen(true)}
      >
        <img className="agent-launcher-mark" src={agentMark} alt="" />
        <span className="agent-launcher-copy">
          <strong>Agent</strong>
          <small>{t("只读 · 搜索")}</small>
        </span>
      </button>
    </aside>
  );
}
