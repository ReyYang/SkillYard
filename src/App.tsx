import { useEffect, useState } from "react";

import { InventoryPage } from "./components/InventoryPage";
import { OnboardingPage } from "./components/OnboardingPage";
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
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [refreshError, setRefreshError] = useState<string | null>(null);

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

  const startInitialScan = async () => {
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

  const refreshLocalInventory = async () => {
    if (isRefreshing) return;
    setIsRefreshing(true);
    setRefreshError(null);
    try {
      const outcome = await client.refreshLocalInventory();
      setViewState({ status: "ready", outcome });
    } catch (error) {
      // 刷新失败不抹掉上次已提交清单，用户仍可继续搜索和浏览。
      setRefreshError(formatError(error));
    } finally {
      setIsRefreshing(false);
    }
  };

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
    return (
      <OnboardingPage
        isScanning={isScanning}
        onStartScan={startInitialScan}
      />
    );
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

  return (
    <InventoryPage
      outcome={viewState.outcome}
      isRefreshing={isRefreshing}
      refreshError={refreshError}
      onRefresh={refreshLocalInventory}
    />
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
