import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));

import { tauriSkillYardClient } from "./skillyardClient";

describe("Tauri IPC contract", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
  });

  it("只通过任务级命令读取启动状态", async () => {
    mocks.invoke.mockResolvedValue({
      type: "onboardingRequired",
      supportedApps: [],
    });

    await tauriSkillYardClient.getStartupState();

    expect(mocks.invoke).toHaveBeenCalledWith("get_startup_state");
  });

  it("语言偏好只通过正式任务级命令读取和保存", async () => {
    mocks.invoke
      .mockResolvedValueOnce({
        type: "preferences",
        language: "zhCn",
        ai: {
          enabled: false,
          disclosureAccepted: false,
          provider: "openAi",
          model: "gpt-5.6-terra",
          hasApiKey: false,
          verified: false,
        },
      })
      .mockResolvedValueOnce({
        type: "preferences",
        language: "en",
        ai: {
          enabled: false,
          disclosureAccepted: false,
          provider: "openAi",
          model: "gpt-5.6-terra",
          hasApiKey: false,
          verified: false,
        },
      });

    await tauriSkillYardClient.getPreferences();
    await tauriSkillYardClient.setInterfaceLanguage("en");

    expect(mocks.invoke).toHaveBeenNthCalledWith(1, "get_preferences");
    expect(mocks.invoke).toHaveBeenNthCalledWith(2, "set_interface_language", {
      language: "en",
    });
  });

  it("AI 配置、Keychain 和连接测试只使用固定任务级命令", async () => {
    const outcome = {
      type: "preferences",
      language: "zhCn",
      ai: {
        enabled: true,
        disclosureAccepted: true,
        provider: "openAi",
        model: "gpt-5.6-terra",
        hasApiKey: true,
        verified: false,
      },
    };
    mocks.invoke.mockResolvedValue(outcome);

    await tauriSkillYardClient.setAiConfiguration({
      enabled: true,
      disclosureAccepted: true,
      provider: "openAi",
      model: "gpt-5.6-terra",
    });
    await tauriSkillYardClient.saveAiApiKey("skillyard-fixture-openai-api-key");
    await tauriSkillYardClient.deleteAiApiKey();
    await tauriSkillYardClient.testAiConnection();

    expect(mocks.invoke).toHaveBeenNthCalledWith(1, "set_ai_configuration", {
      enabled: true,
      disclosureAccepted: true,
      provider: "openAi",
      model: "gpt-5.6-terra",
    });
    expect(mocks.invoke).toHaveBeenNthCalledWith(2, "save_ai_api_key", {
      apiKey: "skillyard-fixture-openai-api-key",
    });
    expect(mocks.invoke).toHaveBeenNthCalledWith(3, "delete_ai_api_key");
    expect(mocks.invoke).toHaveBeenNthCalledWith(4, "test_ai_connection");
  });

  it("全局助手只提交稳定页面身份和内存会话，不提交本机路径", async () => {
    mocks.invoke.mockResolvedValue({
      type: "agentReply",
      reply: "fixture answer",
      localMatchFound: true,
      searchedPublicWeb: false,
      searchResults: [],
    });
    const context = {
      type: "skill" as const,
      inventoryId: "managed:member-1",
    };
    const messages = [
      { role: "user" as const, content: "解释这个 Skill" },
    ];

    await tauriSkillYardClient.askAgent(context, messages);

    expect(mocks.invoke).toHaveBeenCalledWith("ask_agent", {
      context,
      messages,
    });
  });

  it("生成 Skill 说明时只提交稳定 Inventory ID", async () => {
    mocks.invoke.mockResolvedValue({
      type: "inventory",
      scanCompletedAt: 1,
      entries: [],
      supportedApps: [],
      lastLocalRefresh: null,
      scanIssues: [],
      recoveryIssues: [],
      projects: [],
      mounts: [],
      bundleUpdates: [],
    });

    await tauriSkillYardClient.generateSkillAiExplanation("managed:member-1");

    expect(mocks.invoke).toHaveBeenCalledWith(
      "generate_skill_ai_explanation",
      {
        inventoryId: "managed:member-1",
      },
    );
  });

  it("后台 AI 整理不接受前端提交 Skill、文件或任务状态", async () => {
    mocks.invoke.mockResolvedValue({
      type: "inventory",
      scanCompletedAt: 1,
      entries: [],
      supportedApps: [],
      lastLocalRefresh: null,
      scanIssues: [],
      recoveryIssues: [],
      projects: [],
      mounts: [],
      bundleUpdates: [],
    });

    await tauriSkillYardClient.organizeSkillAiExplanations();

    expect(mocks.invoke).toHaveBeenCalledWith(
      "organize_skill_ai_explanations",
    );
  });

  it("打开 Central Store 时不允许前端提交路径", async () => {
    mocks.invoke.mockResolvedValue(undefined);

    await tauriSkillYardClient.openCentralStore();

    expect(mocks.invoke).toHaveBeenCalledWith("open_central_store");
  });

  it("只通过任务级命令开始首次扫描", async () => {
    mocks.invoke.mockResolvedValue({
      type: "inventory",
      scanCompletedAt: 1,
      entries: [],
      supportedApps: [],
      lastLocalRefresh: null,
      scanIssues: [],
      recoveryIssues: [],
    });

    await tauriSkillYardClient.startInitialScan();

    expect(mocks.invoke).toHaveBeenCalledWith("start_initial_scan");
  });

  it("只通过任务级命令刷新本机清单", async () => {
    mocks.invoke.mockResolvedValue({
      type: "inventory",
      scanCompletedAt: 1,
      entries: [],
      supportedApps: [],
      lastLocalRefresh: null,
      scanIssues: [],
      recoveryIssues: [],
    });

    await tauriSkillYardClient.refreshLocalInventory();

    expect(mocks.invoke).toHaveBeenCalledWith("refresh_local_inventory");
  });

  it("检查 Bundle 更新时不提交前端推断参数", async () => {
    mocks.invoke.mockResolvedValue({
      type: "inventory",
      scanCompletedAt: 1,
      entries: [],
      supportedApps: [],
      lastLocalRefresh: null,
      scanIssues: [],
      recoveryIssues: [],
      projects: [],
      mounts: [],
      bundleUpdates: [],
    });

    await tauriSkillYardClient.checkBundleUpdates();

    expect(mocks.invoke).toHaveBeenCalledWith("check_bundle_updates");
  });

  it("只通过任务级命令打开 Source 发现页", async () => {
    mocks.invoke.mockResolvedValue({
      type: "sourceDiscovery",
      sources: [],
      highlightedSourceId: null,
      highlightedMemberPath: null,
    });

    await tauriSkillYardClient.openSourceDiscovery();

    expect(mocks.invoke).toHaveBeenCalledWith("open_source_discovery");
  });

  it("搜索 skills.sh 时只提交用户查询", async () => {
    mocks.invoke.mockResolvedValue({
      type: "skillsShSearch",
      query: "react",
      sources: [],
    });

    await tauriSkillYardClient.searchSkillsSh(" react ");

    expect(mocks.invoke).toHaveBeenCalledWith("search_skills_sh", {
      query: " react ",
    });
  });

  it("重新加载 GitHub Source 时只提交 sourceId", async () => {
    mocks.invoke.mockResolvedValue({
      type: "sourceDiscovery",
      sources: [],
      highlightedSourceId: "source-1",
      highlightedMemberPath: null,
    });

    await tauriSkillYardClient.reloadGithubSource("source-1");

    expect(mocks.invoke).toHaveBeenCalledWith("reload_github_source", {
      sourceId: "source-1",
    });
  });

  it("添加 GitHub Source 时保留明确 ref 或 null", async () => {
    mocks.invoke.mockResolvedValue({
      type: "sourceDiscovery",
      sources: [],
      highlightedSourceId: "source-1",
      highlightedMemberPath: "skills/example",
    });

    await tauriSkillYardClient.addGithubSource(
      "https://github.com/owner/repo/tree/feature/example",
      "feature",
    );
    await tauriSkillYardClient.addGithubSource("owner/repo", null);

    expect(mocks.invoke).toHaveBeenNthCalledWith(1, "add_github_source", {
      input: "https://github.com/owner/repo/tree/feature/example",
      trackedRef: "feature",
    });
    expect(mocks.invoke).toHaveBeenNthCalledWith(2, "add_github_source", {
      input: "owner/repo",
      trackedRef: null,
    });
  });

  it("确认 Tracked Ref 变更时只提交 opaque Plan ID", async () => {
    mocks.invoke.mockResolvedValue({
      type: "sourceDiscovery",
      sources: [],
      highlightedSourceId: "source-1",
      highlightedMemberPath: null,
    });

    await tauriSkillYardClient.confirmSourceRefChange("ref-plan-1");

    expect(mocks.invoke).toHaveBeenCalledWith("confirm_source_ref_change", {
      planId: "ref-plan-1",
    });
  });

  it("补充来源只提交 Bundle、Source 和用户明确的成员对应关系", async () => {
    mocks.invoke.mockResolvedValue({ id: "association-plan-1" });
    const memberChoices = [
      {
        memberId: "member-1",
        sourceRelativePath: "skills/example",
      },
      {
        memberId: "member-local",
        sourceRelativePath: null,
      },
    ];

    await tauriSkillYardClient.createSourceAssociationPlan(
      "bundle-1",
      "source-1",
      memberChoices,
    );

    expect(mocks.invoke).toHaveBeenCalledWith(
      "create_source_association_plan",
      {
        bundleId: "bundle-1",
        sourceId: "source-1",
        memberChoices,
      },
    );
  });

  it("关联与归并使用同一个确认命令，并只提交 Plan 内的内容选择", async () => {
    mocks.invoke.mockResolvedValue({
      type: "inventory",
      scanCompletedAt: 1,
      entries: [],
      supportedApps: [],
      lastLocalRefresh: null,
      scanIssues: [],
      recoveryIssues: [],
      projects: [],
      mounts: [],
    });
    const contentChoices = [
      { conflictId: "conflict-1", memberId: "member-2" },
    ];

    await tauriSkillYardClient.confirmSourceAssociationPlan(
      "association-plan-1",
      contentChoices,
    );

    expect(mocks.invoke).toHaveBeenCalledWith(
      "confirm_source_association_plan",
      {
        planId: "association-plan-1",
        contentChoices,
      },
    );
  });

  it("放弃关联 Plan 时只提交 opaque Plan ID", async () => {
    mocks.invoke.mockResolvedValue(undefined);

    await tauriSkillYardClient.discardSourceAssociationPlan(
      "association-plan-1",
    );

    expect(mocks.invoke).toHaveBeenCalledWith(
      "discard_source_association_plan",
      {
        planId: "association-plan-1",
      },
    );
  });

  it("从 GitHub Source 创建通用安装 Plan 时只提交 sourceId", async () => {
    mocks.invoke.mockResolvedValue({ id: "install-plan-1" });

    await tauriSkillYardClient.createGithubInstallPlan("source-1");

    expect(mocks.invoke).toHaveBeenCalledWith("create_github_install_plan", {
      sourceId: "source-1",
    });
  });

  it("为单个 Bundle 准备更新时只提交稳定 Bundle ID", async () => {
    mocks.invoke.mockResolvedValue({ id: "update-plan-1" });

    await tauriSkillYardClient.createBundleUpdatePlan("bundle-1");

    expect(mocks.invoke).toHaveBeenCalledWith("create_bundle_update_plan", {
      bundleId: "bundle-1",
    });
  });

  it("手动替换 Bundle 时只把稳定 Bundle ID 交给原生选择器", async () => {
    mocks.invoke.mockResolvedValue(null);

    await tauriSkillYardClient.chooseBundleReplacementPlan("bundle-archive");

    expect(mocks.invoke).toHaveBeenCalledWith(
      "choose_bundle_replacement_plan",
      {
        bundleId: "bundle-archive",
      },
    );
  });

  it("检查 Editable Local 时只提交稳定 Bundle ID", async () => {
    mocks.invoke.mockResolvedValue({
      type: "inventory",
      scanCompletedAt: 1,
      entries: [],
      supportedApps: [],
      lastLocalRefresh: null,
      scanIssues: [],
      recoveryIssues: [],
      projects: [],
      mounts: [],
      bundleUpdates: [],
    });

    await tauriSkillYardClient.checkEditableLocalBundle("bundle-editable");

    expect(mocks.invoke).toHaveBeenCalledWith("check_editable_local_bundle", {
      bundleId: "bundle-editable",
    });
  });

  it("准备全部更新时不提交前端推断的 Bundle 列表", async () => {
    mocks.invoke.mockResolvedValue({
      type: "bundleUpdateBatchPlan",
      plan: { id: "batch-plan-1", items: [], createdAt: 1, expiresAt: 2 },
    });

    await tauriSkillYardClient.createBundleUpdateBatchPlan();

    expect(mocks.invoke).toHaveBeenCalledWith(
      "create_bundle_update_batch_plan",
    );
  });

  it("确认全部更新时只提交 Plan ID 和页面顺序中的 Bundle Item ID", async () => {
    mocks.invoke.mockResolvedValue({
      type: "bundleUpdateBatchResult",
      result: {
        id: "batch-1",
        status: "completed",
        items: [],
        confirmedAt: 1,
        updatedAt: 2,
      },
    });

    await tauriSkillYardClient.confirmBundleUpdateBatchPlan("batch-plan-1", [
      "item-beta",
      "item-alpha",
    ]);

    expect(mocks.invoke).toHaveBeenCalledWith(
      "confirm_bundle_update_batch_plan",
      {
        planId: "batch-plan-1",
        selectedItemIds: ["item-beta", "item-alpha"],
      },
    );
  });

  it("返回全部更新预览时只提交需要清理的 Plan ID", async () => {
    mocks.invoke.mockResolvedValue({
      type: "inventory",
      scanCompletedAt: 1,
      entries: [],
      supportedApps: [],
      lastLocalRefresh: null,
      scanIssues: [],
      recoveryIssues: [],
      projects: [],
      mounts: [],
      bundleUpdates: [],
    });

    await tauriSkillYardClient.discardBundleUpdateBatchPlan("batch-plan-1");

    expect(mocks.invoke).toHaveBeenCalledWith(
      "discard_bundle_update_batch_plan",
      {
        planId: "batch-plan-1",
      },
    );
  });

  it("确认已读全部更新结果时把结果 ID 作为 batchId 提交", async () => {
    mocks.invoke.mockResolvedValue({
      type: "inventory",
      scanCompletedAt: 1,
      entries: [],
      supportedApps: [],
      lastLocalRefresh: null,
      scanIssues: [],
      recoveryIssues: [],
      projects: [],
      mounts: [],
      bundleUpdates: [],
    });

    await tauriSkillYardClient.acknowledgeBundleUpdateBatchResult("batch-1");

    expect(mocks.invoke).toHaveBeenCalledWith(
      "acknowledge_bundle_update_batch_result",
      {
        batchId: "batch-1",
      },
    );
  });

  it("准备移除 Project 时只提交稳定 Project ID", async () => {
    mocks.invoke.mockResolvedValue({
      type: "removalPlan",
      plan: { id: "removal-plan-1", kind: "project" },
    });

    await tauriSkillYardClient.createProjectRemovalPlan("project-1");

    expect(mocks.invoke).toHaveBeenCalledWith(
      "create_project_removal_plan",
      {
        projectId: "project-1",
      },
    );
  });

  it("准备删除 Source 时只提交稳定 Source ID", async () => {
    mocks.invoke.mockResolvedValue({
      type: "removalPlan",
      plan: { id: "removal-plan-2", kind: "source" },
    });

    await tauriSkillYardClient.createSourceRemovalPlan("source-1");

    expect(mocks.invoke).toHaveBeenCalledWith("create_source_removal_plan", {
      sourceId: "source-1",
    });
  });

  it("准备删除 Bundle 时只提交稳定 Bundle ID", async () => {
    mocks.invoke.mockResolvedValue({
      type: "removalPlan",
      plan: { id: "removal-plan-3", kind: "bundle" },
    });

    await tauriSkillYardClient.createBundleRemovalPlan("bundle-1");

    expect(mocks.invoke).toHaveBeenCalledWith("create_bundle_removal_plan", {
      bundleId: "bundle-1",
    });
  });

  it("确认 Removal Plan 时只提交 opaque Plan ID", async () => {
    mocks.invoke.mockResolvedValue({
      type: "inventory",
      scanCompletedAt: 1,
      entries: [],
      supportedApps: [],
      lastLocalRefresh: null,
      scanIssues: [],
      recoveryIssues: [],
      projects: [],
      mounts: [],
      bundleUpdates: [],
    });

    await tauriSkillYardClient.confirmRemovalPlan("removal-plan-1");

    expect(mocks.invoke).toHaveBeenCalledWith("confirm_removal_plan", {
      planId: "removal-plan-1",
    });
  });

  it("放弃 Removal Plan 时只提交 opaque Plan ID", async () => {
    mocks.invoke.mockResolvedValue({
      type: "sourceDiscovery",
      sources: [],
      highlightedSourceId: null,
      highlightedMemberPath: null,
    });

    await tauriSkillYardClient.discardRemovalPlan("removal-plan-1");

    expect(mocks.invoke).toHaveBeenCalledWith("discard_removal_plan", {
      planId: "removal-plan-1",
    });
  });

  it("放弃安装 Plan 时只提交 opaque Plan ID", async () => {
    mocks.invoke.mockResolvedValue(undefined);

    await tauriSkillYardClient.discardInstallPlan("install-plan-1");

    expect(mocks.invoke).toHaveBeenCalledWith("discard_install_plan", {
      planId: "install-plan-1",
    });
  });

  it("只通过 Rust 任务命令打开文件夹选择器", async () => {
    mocks.invoke.mockResolvedValue(null);

    await tauriSkillYardClient.chooseFolderInstallPlan();

    expect(mocks.invoke).toHaveBeenCalledWith("choose_folder_install_plan");
  });

  it("归档、直接 URL 和 Editable Local 都使用封闭任务命令", async () => {
    mocks.invoke.mockResolvedValue(null);

    await tauriSkillYardClient.chooseArchiveInstallPlan();
    await tauriSkillYardClient.createUrlInstallPlan(
      "https://example.com/skills.zip",
    );
    await tauriSkillYardClient.chooseEditableLocalInstallPlan();

    expect(mocks.invoke).toHaveBeenNthCalledWith(
      1,
      "choose_archive_install_plan",
    );
    expect(mocks.invoke).toHaveBeenNthCalledWith(2, "create_url_install_plan", {
      url: "https://example.com/skills.zip",
    });
    expect(mocks.invoke).toHaveBeenNthCalledWith(
      3,
      "choose_editable_local_install_plan",
    );
  });

  it("Editable Local 重新关联只传 Source ID 或 opaque Plan ID", async () => {
    mocks.invoke.mockResolvedValue(null);

    await tauriSkillYardClient.chooseEditableLocalRelinkPlan("source-1");
    await tauriSkillYardClient.confirmEditableLocalRelinkPlan("relink-plan-1");
    await tauriSkillYardClient.discardEditableLocalRelinkPlan("relink-plan-2");

    expect(mocks.invoke).toHaveBeenNthCalledWith(
      1,
      "choose_editable_local_relink_plan",
      { sourceId: "source-1" },
    );
    expect(mocks.invoke).toHaveBeenNthCalledWith(
      2,
      "confirm_editable_local_relink_plan",
      { planId: "relink-plan-1" },
    );
    expect(mocks.invoke).toHaveBeenNthCalledWith(
      3,
      "discard_editable_local_relink_plan",
      { planId: "relink-plan-2" },
    );
  });

  it("确认时只把 opaque Plan 与候选 ID 交回 Rust", async () => {
    mocks.invoke.mockResolvedValue({
      type: "inventory",
      scanCompletedAt: 1,
      entries: [],
      supportedApps: [],
      lastLocalRefresh: null,
      scanIssues: [],
      recoveryIssues: [],
    });

    await tauriSkillYardClient.confirmInstallPlan("plan-1", ["candidate-1"]);

    expect(mocks.invoke).toHaveBeenCalledWith("confirm_install_plan", {
      planId: "plan-1",
      selectedCandidateIds: ["candidate-1"],
    });
  });

  it("先选择 Project，再通过独立命令提交确认后的路径", async () => {
    mocks.invoke
      .mockResolvedValueOnce({
        displayName: "project",
        rootPath: "/tmp/project",
      })
      .mockResolvedValueOnce({ type: "inventory" });

    await tauriSkillYardClient.chooseProjectDirectory();
    await tauriSkillYardClient.registerProject("/tmp/project");

    expect(mocks.invoke).toHaveBeenNthCalledWith(1, "choose_project_directory");
    expect(mocks.invoke).toHaveBeenNthCalledWith(2, "register_project", {
      rootPath: "/tmp/project",
    });
  });

  it("创建 Takeover Plan 时提交完整且明确的用户选择", async () => {
    mocks.invoke.mockResolvedValue({ id: "takeover-plan-1" });
    const request = {
      members: [
        {
          observationIds: ["origin-1", "origin-2"],
          selectedObservationId: "origin-2",
          preservedObservationIds: ["origin-1"],
        },
      ],
      sharedTargets: [
        { sharedObservationId: "origin-2", appId: "claudeCode" as const },
      ],
    };

    await tauriSkillYardClient.createTakeoverPlan(request);

    expect(mocks.invoke).toHaveBeenCalledWith("create_takeover_plan", {
      request,
    });
  });

  it("确认 Takeover 时只提交 opaque Plan ID", async () => {
    mocks.invoke.mockResolvedValue({
      type: "inventory",
      scanCompletedAt: 1,
      entries: [],
      supportedApps: [],
      lastLocalRefresh: null,
      scanIssues: [],
      recoveryIssues: [],
      projects: [],
      mounts: [],
    });

    await tauriSkillYardClient.confirmTakeoverPlan("takeover-plan-1");

    expect(mocks.invoke).toHaveBeenCalledWith("confirm_takeover_plan", {
      planId: "takeover-plan-1",
    });
  });

  it("创建 Mount Plan 时提交明确的 member、应用和 scope", async () => {
    mocks.invoke.mockResolvedValue({ id: "mount-plan-1" });

    await tauriSkillYardClient.createMountPlan(
      "member-1",
      "codex",
      "project",
      "project-1",
    );

    expect(mocks.invoke).toHaveBeenCalledWith("create_mount_plan", {
      memberId: "member-1",
      appId: "codex",
      scope: "project",
      projectId: "project-1",
    });
  });

  it("移除 Mount 也必须先创建 opaque Plan", async () => {
    mocks.invoke.mockResolvedValue({ id: "remove-plan-1" });

    await tauriSkillYardClient.createRemoveMountPlan("mount-1");

    expect(mocks.invoke).toHaveBeenCalledWith("create_remove_mount_plan", {
      mountId: "mount-1",
    });
  });

  it("修复 Mount 也必须先创建 opaque Plan", async () => {
    mocks.invoke.mockResolvedValue({ id: "repair-plan-1" });

    await tauriSkillYardClient.createRepairMountPlan("mount-1");

    expect(mocks.invoke).toHaveBeenCalledWith("create_repair_mount_plan", {
      mountId: "mount-1",
    });
  });

  it("确认 Mount 事务时只提交 opaque Plan ID", async () => {
    mocks.invoke.mockResolvedValue({
      type: "inventory",
      scanCompletedAt: 1,
      entries: [],
      supportedApps: [],
      lastLocalRefresh: null,
      scanIssues: [],
      recoveryIssues: [],
      projects: [],
      mounts: [],
    });

    await tauriSkillYardClient.confirmMountPlan("mount-plan-1");

    expect(mocks.invoke).toHaveBeenCalledWith("confirm_mount_plan", {
      planId: "mount-plan-1",
    });
  });

  it("创建 Batch Mount Plan 时只提交 Bundle 与明确目标请求", async () => {
    mocks.invoke.mockResolvedValue({ id: "batch-plan-1" });
    const requests = [
      {
        memberId: "member-1",
        appId: "codex" as const,
        scope: "global" as const,
        projectId: null,
      },
      {
        memberId: "member-2",
        appId: "claudeCode" as const,
        scope: "project" as const,
        projectId: "project-1",
      },
    ];

    await tauriSkillYardClient.createBatchMountPlan("bundle-1", requests);

    expect(mocks.invoke).toHaveBeenCalledWith("create_batch_mount_plan", {
      bundleId: "bundle-1",
      requests,
    });
  });

  it("确认 Batch Mount 时只提交 opaque Plan 与最终选中项", async () => {
    mocks.invoke.mockResolvedValue({
      type: "inventory",
      scanCompletedAt: 1,
      entries: [],
      supportedApps: [],
      lastLocalRefresh: null,
      scanIssues: [],
      recoveryIssues: [],
      projects: [],
      mounts: [],
    });

    await tauriSkillYardClient.confirmBatchMountPlan("batch-plan-1", [
      "batch-item-1",
      "batch-item-3",
    ]);

    expect(mocks.invoke).toHaveBeenCalledWith("confirm_batch_mount_plan", {
      planId: "batch-plan-1",
      selectedItemIds: ["batch-item-1", "batch-item-3"],
    });
  });
});
