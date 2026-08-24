import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { UiOutcome } from "../domain";
import { I18nProvider } from "../i18n";
import { DiscoverPage } from "./DiscoverPage";

type DiscoverOutcome = Extract<UiOutcome, { type: "discover" }>;
type DiscoverWebSearchOutcome = Extract<
  UiOutcome,
  { type: "discoverWebSearch" }
>;

describe("DiscoverPage", () => {
  it("输入只筛选本机与已保存 Source 目录，并始终保留三个结果分区", async () => {
    const user = userEvent.setup();
    const onSearchWeb = vi.fn();
    const onOpenSourceManagement = vi.fn();
    render(
      <I18nProvider language="zhCn">
        <DiscoverPage
          outcome={discoverOutcome()}
          webSearch={null}
          isSearchingWeb={false}
          webSearchError={null}
          onBack={vi.fn()}
          onSearchWeb={onSearchWeb}
          onOpenExternalUrl={vi.fn()}
          onPreviewInstall={vi.fn()}
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
    expect(onSearchWeb).not.toHaveBeenCalled();
    expect(onOpenSourceManagement).not.toHaveBeenCalled();

    await user.clear(input);
    await user.type(input, "公开资料");
    expect(
      screen.getByRole("article", { name: "example/research" }),
    ).toBeInTheDocument();
    expect(screen.getByText("research")).toBeInTheDocument();
    expect(
      screen.queryByRole("article", { name: "tdd" }),
    ).not.toBeInTheDocument();
    expect(screen.getByText("尚未安装")).toBeInTheDocument();

    expect(screen.getByText("目录尚未加载")).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", {
        name: "前往 Source 管理：vercel-labs/skills",
      }),
    );
    expect(onOpenSourceManagement).toHaveBeenCalledWith(
      "source-unloaded",
      null,
    );
  });

  it("把所有本机管理归属作为只读结果呈现，并清楚显示本地空结果", async () => {
    const user = userEvent.setup();
    const onSearchWeb = vi.fn();
    const outcome = discoverOutcome();
    outcome.localSkills = [
      {
        ...outcome.localSkills[0],
        inventoryId: "inventory-managed",
        skillName: "managed",
        description: "本机只读管理测试",
        managementKind: "skillYardManaged",
      },
      {
        ...outcome.localSkills[0],
        inventoryId: "inventory-takeover",
        skillName: "takeover",
        description: "本机只读管理测试",
        managementKind: "takeoverCandidate",
      },
      {
        ...outcome.localSkills[0],
        inventoryId: "inventory-agent",
        skillName: "agent-owned",
        description: "本机只读管理测试",
        managementKind: "agentManaged",
      },
      {
        ...outcome.localSkills[0],
        inventoryId: "inventory-project",
        skillName: "project-owned",
        description: "本机只读管理测试",
        managementKind: "projectManaged",
      },
    ];
    outcome.sources = [outcome.sources[0]];

    render(
      <I18nProvider language="zhCn">
        <DiscoverPage
          outcome={outcome}
          webSearch={null}
          isSearchingWeb={false}
          webSearchError={null}
          onBack={vi.fn()}
          onSearchWeb={onSearchWeb}
          onOpenExternalUrl={vi.fn()}
          onPreviewInstall={vi.fn()}
          onOpenSourceManagement={vi.fn()}
        />
      </I18nProvider>,
    );

    const input = screen.getByRole("searchbox", { name: "搜索 Skill" });
    await user.type(input, "只读管理");
    const localRegion = screen.getByRole("region", { name: "本机已有" });
    expect(localRegion).toHaveTextContent("由 SkillYard 管理");
    expect(localRegion).toHaveTextContent("待接管");
    expect(localRegion).toHaveTextContent("其他管理方");
    expect(localRegion).toHaveTextContent("项目仓库管理");
    expect(onSearchWeb).not.toHaveBeenCalled();

    await user.clear(input);
    await user.type(input, "没有匹配");
    expect(screen.getByText("本机没有匹配的 Skill")).toBeInTheDocument();
    expect(
      screen.getByText("已加载的 Source 中没有匹配成员"),
    ).toBeInTheDocument();
    expect(screen.getByText("尚未提交全网搜索")).toBeInTheDocument();
    expect(onSearchWeb).not.toHaveBeenCalled();
  });

  it("主动提交始终搜索全网，并把同一 canonical Source 的本机、Source 与线上事实合并为一张卡片", async () => {
    const user = userEvent.setup();
    const onSearchWeb = vi.fn();
    const onOpenExternalUrl = vi.fn();
    const onPreviewInstall = vi.fn();
    const outcome = discoverOutcome();
    outcome.localSkills[0] = {
      ...outcome.localSkills[0],
      managementKind: "skillYardManaged",
      bundleId: "bundle-research",
      bundleDisplayName: "example/research",
      sourceId: "source-loaded",
      sourceCanonicalIdentity: "github:example/research",
      sourceDisplayName: "example/research",
    };
    const webSearch: DiscoverWebSearchOutcome = {
      type: "discoverWebSearch",
      query: "公开资料",
      results: [
        {
          title: "Example Research",
          url: "https://github.com/example/research",
          kind: "github",
          canonicalIdentity: "github:example/research",
          existingSourceId: "source-loaded",
        },
        {
          title: "Research discussion",
          url: "https://forum.example.com/research",
          kind: "reference",
          canonicalIdentity: null,
          existingSourceId: null,
        },
      ],
    };

    render(
      <I18nProvider language="zhCn">
        <DiscoverPage
          outcome={outcome}
          webSearch={webSearch}
          isSearchingWeb={false}
          webSearchError={null}
          onBack={vi.fn()}
          onSearchWeb={onSearchWeb}
          onOpenSourceManagement={vi.fn()}
          onOpenExternalUrl={onOpenExternalUrl}
          onPreviewInstall={onPreviewInstall}
        />
      </I18nProvider>,
    );

    const input = screen.getByRole("searchbox", { name: "搜索 Skill" });
    await user.type(input, "公开资料");
    await user.click(screen.getByRole("button", { name: "搜索全网" }));
    expect(onSearchWeb).toHaveBeenCalledWith("公开资料");
    expect(
      screen.getAllByRole("article", { name: "example/research" }),
    ).toHaveLength(1);
    expect(
      screen.getByRole("region", { name: "本机已有" }),
    ).toHaveTextContent("已经安装");
    expect(
      screen.getByRole("region", { name: "已添加来源" }),
    ).not.toHaveTextContent("example/research");
    expect(
      screen.getByRole("region", { name: "全网发现" }),
    ).not.toHaveTextContent("Example Research");

    await user.click(
      screen.getByRole("button", { name: "打开 Research discussion" }),
    );
    expect(onOpenExternalUrl).toHaveBeenCalledWith(
      "https://forum.example.com/research",
    );
    expect(onPreviewInstall).not.toHaveBeenCalled();
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
        sourceId: null,
        sourceCanonicalIdentity: null,
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
