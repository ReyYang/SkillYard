import { useRef, useState, type FormEvent } from "react";

import type {
  AgentConversationMessage,
  AgentPageContext,
  AgentReply,
  AiPreferences,
} from "../domain";
import { useI18n } from "../i18n";

interface AgentOverlayProps {
  context: AgentPageContext;
  aiPreferences: AiPreferences;
  onAsk(
    context: AgentPageContext,
    messages: AgentConversationMessage[],
  ): Promise<AgentReply>;
}

export function AgentOverlay({
  context,
  aiPreferences,
  onAsk,
}: AgentOverlayProps) {
  const { t } = useI18n();
  const [isOpen, setIsOpen] = useState(false);
  const [messages, setMessages] = useState<AgentConversationMessage[]>([]);
  const [draft, setDraft] = useState("");
  const [isSending, setIsSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const sessionGeneration = useRef(0);
  const isReady =
    aiPreferences.enabled &&
    aiPreferences.disclosureAccepted &&
    aiPreferences.hasApiKey &&
    aiPreferences.verified;

  const closeSession = () => {
    // 明确关闭就是 Session 的唯一销毁点；失焦和页面导航不触发这里。
    sessionGeneration.current += 1;
    setIsOpen(false);
    setMessages([]);
    setDraft("");
    setError(null);
    setIsSending(false);
  };

  const send = async (event: FormEvent) => {
    event.preventDefault();
    const question = draft.trim();
    if (!question || isSending || !isReady) return;
    const nextMessages = [
      ...messages,
      { role: "user", content: question } satisfies AgentConversationMessage,
    ];
    setMessages(nextMessages);
    setDraft("");
    setError(null);
    setIsSending(true);
    const activeGeneration = sessionGeneration.current;
    try {
      const reply = await onAsk(context, nextMessages);
      if (sessionGeneration.current !== activeGeneration) return;
      setMessages((current) => [
        ...current,
        { role: "assistant", content: reply.reply },
      ]);
    } catch (cause) {
      if (sessionGeneration.current !== activeGeneration) return;
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
        setIsSending(false);
      }
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
            <div>
              <p className="section-eyebrow">SKILLYARD · ASSIST</p>
              <h2>{t("SkillYard 助手")}</h2>
            </div>
            <button
              className="agent-close"
              type="button"
              aria-label={t("关闭 SkillYard 助手")}
              onClick={closeSession}
            >
              ×
            </button>
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
              messages.map((message, index) => (
                <div
                  className={`agent-message ${message.role}`}
                  key={`${message.role}-${index}`}
                >
                  <span>
                    {message.role === "user" ? t("你") : t("SkillYard")}
                  </span>
                  <p>{message.content}</p>
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
              <textarea
                aria-label={t("向 SkillYard 提问")}
                value={draft}
                disabled={!isReady || isSending}
                placeholder={t("问问这个 Skill 能做什么…")}
                rows={3}
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
        <svg viewBox="0 0 48 48" aria-hidden="true">
          <path d="M11 37V15.5C11 12.5 13.5 10 16.5 10H31" />
          <path d="M18 37V22.5C18 19.5 20.5 17 23.5 17H37" />
          <path d="M25 37V29.5C25 26.5 27.5 24 30.5 24H37" />
          <circle cx="11" cy="37" r="2.5" />
          <circle cx="37" cy="17" r="2.5" />
        </svg>
      </button>
    </aside>
  );
}
