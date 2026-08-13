import { describe, expect, it, vi } from "vitest";

vi.mock("./main", () => ({}));

describe("Ticket 8 浏览器视觉夹具", () => {
  it("锁定五组、ID、Mount 与更新状态的精确生产语义", async () => {
    const { assertVisualFixture, visualFixtureInventory } = await import(
      "./visual-fixture"
    );

    expect(() => assertVisualFixture(visualFixtureInventory)).not.toThrow();

    const wrongGroups = structuredClone(visualFixtureInventory);
    wrongGroups.entries[0]!.bundleDisplayName = "anthropics/skills";
    expect(() => assertVisualFixture(wrongGroups)).toThrow(/分组/);

    const duplicateIds = structuredClone(visualFixtureInventory);
    duplicateIds.entries[1]!.id = duplicateIds.entries[0]!.id;
    expect(() => assertVisualFixture(duplicateIds)).toThrow(/ID/);

    const unhealthyMount = structuredClone(visualFixtureInventory);
    unhealthyMount.mounts[0]!.health = "missing";
    expect(() => assertVisualFixture(unhealthyMount)).toThrow(/Mount/);

    const wrongMountApp = structuredClone(visualFixtureInventory);
    wrongMountApp.mounts[0]!.appId = "codex";
    expect(() => assertVisualFixture(wrongMountApp)).toThrow(/Mount/);

    const wrongUpdate = structuredClone(visualFixtureInventory);
    wrongUpdate.bundleUpdates[0]!.status = "upToDate";
    expect(() => assertVisualFixture(wrongUpdate)).toThrow(/更新/);
  });

  it("只给生产可投影的受管成员提供 SKILL.md 描述", async () => {
    const { visualFixtureInventory } = await import("./visual-fixture");
    const managed = visualFixtureInventory.entries.find(
      (entry) => entry.managementKind === "skillYardManaged",
    );
    const takeover = visualFixtureInventory.entries.find(
      (entry) => entry.managementKind === "takeoverCandidate",
    );
    const official = visualFixtureInventory.entries.find(
      (entry) => entry.managementKind === "agentManaged",
    );

    expect(managed?.description).toBeTruthy();
    expect(takeover?.description).toBeNull();
    expect(official?.description).toBeNull();
  });
});
