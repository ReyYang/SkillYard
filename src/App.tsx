import { useEffect, useState } from "react";

import type { UiOutcome } from "./domain";
import {
  tauriSkillYardClient,
  type SkillYardClient,
} from "./skillyardClient";

interface AppProps {
  client?: SkillYardClient;
}

type ViewState =
  | { status: "loading" }
  | { status: "ready"; outcome: UiOutcome }
  | { status: "error"; message: string };

export function App({ client = tauriSkillYardClient }: AppProps) {
  const [viewState, setViewState] = useState<ViewState>({ status: "loading" });
  const [isScanning, setIsScanning] = useState(false);

  useEffect(() => {
    let active = true;
    client.getStartupState().then(
      (outcome) => {
        if (active) setViewState({ status: "ready", outcome });
      },
      (error: unknown) => {
        if (active) setViewState({ status: "error", message: formatError(error) });
      },
    );
    return () => {
      active = false;
    };
  }, [client]);

  if (viewState.status === "loading") {
    return (
      <main className="state-page" aria-label="SkillYard 正在启动">
        <span className="spinner" aria-hidden="true" />
        <p>正在读取本机状态…</p>
      </main>
    );
  }
  if (viewState.status === "error") {
    return (
      <main className="state-page" role="alert">
        <p className="eyebrow">SKILLYARD · LOCAL ERROR</p>
        <h1>暂时无法继续</h1>
        <p>{viewState.message}</p>
      </main>
    );
  }
  if (viewState.outcome.type === "onboardingRequired") {
    const startScan = async () => {
      if (isScanning) return;
      setIsScanning(true);
      try {
        const outcome = await client.startInitialScan();
        setViewState({ status: "ready", outcome });
      } catch (error) {
        setViewState({ status: "error", message: formatError(error) });
      } finally {
        setIsScanning(false);
      }
    };
    return <OnboardingPage isScanning={isScanning} onStartScan={startScan} />;
  }

  if (viewState.outcome.type === "unsupportedPlatform") {
    return (
      <main className="state-page">
        <p className="eyebrow">SKILLYARD · PLATFORM CHECK</p>
        <h1>当前 Mac 不受 SkillYard 1.0 支持</h1>
        <p>
          需要 macOS {viewState.outcome.minimumMajorVersion} 或更高版本的 Apple
          Silicon Mac。
        </p>
      </main>
    );
  }

  return <InventoryPage outcome={viewState.outcome} />;
}

interface OnboardingPageProps {
  isScanning: boolean;
  onStartScan(): void;
}

function OnboardingPage({ isScanning, onStartScan }: OnboardingPageProps) {
  return (
    <main className="onboarding-shell">
      <section className="onboarding-copy">
        <div className="brand-mark" aria-hidden="true">
          SY
        </div>
        <p className="eyebrow">SKILLYARD · LOCAL SKILL LIBRARY</p>
        <h1>管理本机 Skill，从一次只读扫描开始</h1>
        <p className="lead">
          SkillYard 将读取 Codex、Claude Code 和 GitHub Copilot
          已确认的本地 Skill 目录。
        </p>
        <div className="safety-note">
          <span aria-hidden="true">✓</span>
          <p>扫描不会自动接管、移动、覆盖或删除任何 Skill。</p>
        </div>
        <button
          className="primary-action"
          type="button"
          disabled={isScanning}
          onClick={onStartScan}
        >
          {isScanning ? "正在扫描…" : "开始扫描"}
        </button>
      </section>
      <aside className="scope-card" aria-label="扫描范围">
        <p className="scope-label">本次读取范围</p>
        <ul>
          <li>
            <span>Codex</span>
            <code>~/.codex/skills</code>
          </li>
          <li>
            <span>Claude Code</span>
            <code>~/.claude/skills</code>
          </li>
          <li>
            <span>GitHub Copilot</span>
            <code>~/.copilot/skills</code>
          </li>
          <li>
            <span>共享只读目录</span>
            <code>~/.agents/skills</code>
          </li>
        </ul>
        <p className="local-only">全部数据只保存在这台 Mac。</p>
      </aside>
    </main>
  );
}

function InventoryPage({ outcome }: { outcome: Extract<UiOutcome, { type: "inventory" }> }) {
  return (
    <main className="inventory-shell">
      <p className="eyebrow">SKILLYARD · LOCAL INVENTORY</p>
      <h1>
        {outcome.entries.length === 0
          ? "未发现 Skill"
          : `已找到 ${outcome.entries.length} 个 Skill`}
      </h1>
      <p className="lead">结果已保存在本机。SkillYard 没有接管或移动这些内容。</p>
      <ul className="inventory-list">
        {outcome.entries.map((entry) => (
          <li key={entry.id}>
            <strong>{entry.skillName}</strong>
            <code>{entry.skillRoot}</code>
          </li>
        ))}
      </ul>
    </main>
  );
}

function formatError(error: unknown): string {
  if (error instanceof Error) return error.message;
  // Tauri command 会把 Rust 的结构化 UiError 作为普通对象传给前端。
  if (
    typeof error === "object" &&
    error !== null &&
    "message" in error &&
    typeof error.message === "string"
  ) {
    return error.message;
  }
  return "无法读取 SkillYard 状态";
}
