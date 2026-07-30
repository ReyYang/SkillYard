import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { UiOutcome } from "../domain";
import { I18nProvider } from "../i18n";
import { DiscoverPage } from "./DiscoverPage";

type DiscoverOutcome = Extract<UiOutcome, { type: "discover" }>;

describe("DiscoverPage", () => {
  it("输入只筛选本机与已保存 Source 目录，并始终保留三个结果分区", async () => {
    const user = userEvent.setup();
    const onOpenSourceManagement = vi.fn();
    render(
      <I18nProvider language="zhCn">
        <DiscoverPage
          outcome={discoverOutcome()}
          onBack={vi.fn()}
          onOpenSourceManagement={onOpenSourceManagement}
        />
      </I18nProvider>,
    );

    expect(
      screen.getByRole("heading", { name: "本机已有" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "已添加来源" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "全网发现" }),
    ).toBeInTheDocument();
    expect(screen.getByText("尚未提交全网搜索")).toBeInTheDocument();

    const input = screen.getByRole("searchbox", { name: "搜索 Skill" });
    await user.type(input, "测试驱动");
    expect(screen.getByRole("article", { name: "tdd" })).toBeInTheDocument();
    expect(
      screen.queryByRole("article", { name: "research" }),
    ).not.toBeInTheDocument();

    await user.clear(input);
    await user.type(input, "公开资料");
    expect(
      screen.getByRole("article", { name: "research" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("article", { name: "tdd" }),
    ).not.toBeInTheDocument();
    expect(screen.getByText("尚未安装")).toBeInTheDocument();

    expect(screen.getByText("目录尚未加载")).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "管理 vercel-labs/skills" }),
    );
    expect(onOpenSourceManagement).toHaveBeenCalledWith(
      "source-unloaded",
      null,
    );
  });
});

function discoverOutcome(): DiscoverOutcome {
  return {
    type: "discover",
    localSkills: [
      {
        inventoryId: "inventory-tdd",
        skillName: "tdd",
        description: "测试驱动开发工作流",
        aiSummary: "从失败测试开始实现功能。",
        managementKind: "takeoverCandidate",
        bundleId: null,
        bundleDisplayName: null,
        sourceDisplayName: null,
      },
    ],
    sources: [
      {
        id: "source-loaded",
        kind: "github",
        canonicalIdentity: "github:example/research",
        displayName: "example/research",
        locator: "https://github.com/example/research",
        trackedRef: "main",
        memberPathHint: null,
        catalogStatus: "fresh",
        catalogMarker: "fixture-marker",
        catalogFetchedAt: 100,
        lastReloadAt: 100,
        lastReloadError: null,
        bundleId: null,
        adoptedMarker: null,
        members: [
          {
            id: "source-member-research",
            relativePath: "skills/research",
            skillName: "research",
            description: "研究公开资料并整理结论",
            selectable: true,
            validationErrors: [],
            warnings: [],
            installedMemberId: null,
          },
        ],
      },
      {
        id: "source-unloaded",
        kind: "github",
        canonicalIdentity: "github:vercel-labs/skills",
        displayName: "vercel-labs/skills",
        locator: "https://github.com/vercel-labs/skills",
        trackedRef: "main",
        memberPathHint: null,
        catalogStatus: "unloaded",
        catalogMarker: null,
        catalogFetchedAt: null,
        lastReloadAt: null,
        lastReloadError: null,
        bundleId: null,
        adoptedMarker: null,
        members: [],
      },
    ],
  };
}
