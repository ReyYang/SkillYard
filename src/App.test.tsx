import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { App } from "./App";
import type { SkillYardClient } from "./skillyardClient";

describe("首次使用", () => {
  it("先解释扫描边界，并且不会在页面挂载时自动扫描", async () => {
    const client: SkillYardClient = {
      getStartupState: vi.fn().mockResolvedValue({
        type: "onboardingRequired",
        supportedApps: [
          { id: "codex", displayName: "Codex", detected: null },
          { id: "claudeCode", displayName: "Claude Code", detected: null },
          {
            id: "gitHubCopilot",
            displayName: "GitHub Copilot",
            detected: null,
          },
        ],
      }),
      startInitialScan: vi.fn(),
    };

    render(<App client={client} />);

    expect(
      await screen.findByRole("heading", {
        name: "管理本机 Skill，从一次只读扫描开始",
      }),
    ).toBeInTheDocument();
    expect(screen.getByText(/不会自动接管、移动、覆盖或删除/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "开始扫描" })).toBeEnabled();
    expect(client.startInitialScan).not.toHaveBeenCalled();
  });

  it("只在用户点击后扫描，并阻止重复提交", async () => {
    const user = userEvent.setup();
    let finishScan: ((outcome: Awaited<ReturnType<SkillYardClient["startInitialScan"]>>) => void) | undefined;
    const client: SkillYardClient = {
      getStartupState: vi.fn().mockResolvedValue({
        type: "onboardingRequired",
        supportedApps: [],
      }),
      startInitialScan: vi.fn().mockImplementation(
        () =>
          new Promise((resolve) => {
            finishScan = resolve;
          }),
      ),
    };
    render(<App client={client} />);

    const start = await screen.findByRole("button", { name: "开始扫描" });
    await user.click(start);

    expect(client.startInitialScan).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "正在扫描…" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "正在扫描…" }));
    expect(client.startInitialScan).toHaveBeenCalledTimes(1);

    await act(async () => {
      finishScan?.({
        type: "inventory",
        scanCompletedAt: 1_753_000_000_000,
        entries: [
          {
            id: "app_global:/tmp/example",
            skillName: "example",
            declaredName: "example",
            skillRoot: "/tmp/example",
            skillFile: "/tmp/example/SKILL.md",
            locationKind: "appGlobal",
            metadataStatus: "valid",
            observedBy: ["codex"],
          },
        ],
        supportedApps: [
          { id: "codex", displayName: "Codex", detected: true },
        ],
      });
    });

    expect(
      screen.getByRole("heading", { name: "已找到 1 个 Skill" }),
    ).toBeInTheDocument();
    expect(screen.getByText("example")).toBeInTheDocument();
    expect(screen.getByText("/tmp/example")).toBeInTheDocument();
  });

  it("空扫描结果仍然进入已完成清单", async () => {
    const user = userEvent.setup();
    const client: SkillYardClient = {
      getStartupState: vi.fn().mockResolvedValue({
        type: "onboardingRequired",
        supportedApps: [],
      }),
      startInitialScan: vi.fn().mockResolvedValue({
        type: "inventory",
        scanCompletedAt: 1_753_000_000_000,
        entries: [],
        supportedApps: [],
      }),
    };

    render(<App client={client} />);
    await user.click(await screen.findByRole("button", { name: "开始扫描" }));

    expect(
      await screen.findByRole("heading", { name: "未发现 Skill" }),
    ).toBeInTheDocument();
  });

  it("扫描失败时显示 Rust 返回的结构化错误", async () => {
    const user = userEvent.setup();
    const client: SkillYardClient = {
      getStartupState: vi.fn().mockResolvedValue({
        type: "onboardingRequired",
        supportedApps: [],
      }),
      startInitialScan: vi.fn().mockRejectedValue({
        code: "scanError",
        message: "无法读取扫描根目录",
      }),
    };

    render(<App client={client} />);
    await user.click(await screen.findByRole("button", { name: "开始扫描" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "无法读取扫描根目录",
    );
  });
});

describe("返回使用", () => {
  it("直接显示已保存清单，不调用扫描", async () => {
    const client: SkillYardClient = {
      getStartupState: vi.fn().mockResolvedValue({
        type: "inventory",
        scanCompletedAt: 1_753_000_000_000,
        entries: [
          {
            id: "app_global:/tmp/saved",
            skillName: "saved",
            declaredName: "saved",
            skillRoot: "/tmp/saved",
            skillFile: "/tmp/saved/SKILL.md",
            locationKind: "appGlobal",
            metadataStatus: "valid",
            observedBy: ["claudeCode"],
          },
        ],
        supportedApps: [],
      }),
      startInitialScan: vi.fn(),
    };

    render(<App client={client} />);

    expect(
      await screen.findByRole("heading", { name: "已找到 1 个 Skill" }),
    ).toBeInTheDocument();
    expect(client.startInitialScan).not.toHaveBeenCalled();
  });

  it("在不支持的平台显示阻塞页", async () => {
    const client: SkillYardClient = {
      getStartupState: vi.fn().mockResolvedValue({
        type: "unsupportedPlatform",
        actualOs: "macos",
        actualArchitecture: "x86_64",
        actualMajorVersion: 13,
        requiredArchitecture: "aarch64",
        minimumMajorVersion: 14,
      }),
      startInitialScan: vi.fn(),
    };

    render(<App client={client} />);

    expect(
      await screen.findByRole("heading", {
        name: "当前 Mac 不受 SkillYard 1.0 支持",
      }),
    ).toBeInTheDocument();
    expect(client.startInitialScan).not.toHaveBeenCalled();
  });
});
