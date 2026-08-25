import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import path from "node:path";

const { test } = process.env.VITEST
  ? await import("vitest")
  : await import("node:test");
const repoRoot = process.cwd();

async function readRepoFile(relativePath) {
  return readFile(path.join(repoRoot, relativePath), "utf8");
}

function recipeBlock(source, name) {
  const lines = source.split("\n");
  const start = lines.findIndex((line) =>
    new RegExp(`^${name}(?:\\s+[^:]*)?:`).test(line),
  );
  assert.notEqual(start, -1, `missing recipe: ${name}`);

  let end = lines.length;
  for (let index = start + 1; index < lines.length; index += 1) {
    if (/^[A-Za-z_][A-Za-z0-9_-]*(?:\s+[^:]*)?:/.test(lines[index])) {
      end = index;
      break;
    }
  }
  return lines.slice(start, end).join("\n");
}

function section(source, heading) {
  const start = source.indexOf(heading);
  assert.notEqual(start, -1, `missing section: ${heading}`);
  const rest = source.slice(start + heading.length);
  const next = rest.search(/^## /m);
  return next === -1 ? rest : rest.slice(0, next);
}

test("justfile exposes one fail-closed canonical engineering command surface", async () => {
  const justfile = await readRepoFile("justfile");
  assert.match(
    justfile,
    /^set shell := \["bash", "-euo", "pipefail", "-c"\]$/m,
  );

  const publicRecipes = [...justfile.matchAll(/^([a-z][a-z0-9-]*)(?:\s+[^:]*)?:/gm)]
    .map((match) => match[1])
    .filter((name) => name !== "set")
    .sort();
  assert.deepEqual(publicRecipes, [
    "default",
    "format",
    "frontend",
    "mac-contract",
    "migration",
    "release",
    "rust-test",
    "slice",
    "stage",
    "target-report",
    "wire",
  ]);
  assert.doesNotMatch(justfile, /CARGO_INCREMENTAL/);
  assert.doesNotMatch(justfile, /\bcargo\s+clean\b|\brm\s+-|\bunlink\b/);

  const defaultRecipe = recipeBlock(justfile, "default");
  assert.match(defaultRecipe, /just.*--list/);
  assert.doesNotMatch(defaultRecipe, /cargo|pnpm|node\s+--test/);

  const frontend = recipeBlock(justfile, "frontend");
  assert.match(frontend, /pnpm typecheck/);
  assert.match(frontend, /pnpm test/);

  const format = recipeBlock(justfile, "format");
  assert.match(format, /cargo fmt --all -- --check/);
  assert.doesNotMatch(format, /--write|cargo fmt --all\s*$/m);

  const rustTest = recipeBlock(justfile, "rust-test");
  assert.match(rustTest, /^rust-test selector:/m);
  assert.match(rustTest, /\^\[A-Za-z_\].*::/);
  assert.match(rustTest, /cargo test --workspace --locked --test all -- --list/);
  assert.match(rustTest, /CARGO_NET_OFFLINE=true/);
  assert.match(rustTest, /matches.*-eq 1/);
  assert.match(rustTest, /cargo test --workspace --locked --test all/);
  assert.match(rustTest, /-- --exact/);
  assert.doesNotMatch(rustTest, /tauri build|--test\s+\{\{|\$@/);

  const wire = recipeBlock(justfile, "wire");
  assert.match(wire, /pnpm exec vitest run src\/skillyardClient\.test\.ts/);
  assert.match(wire, /cargo test --workspace --locked --lib domain::tests:: -- --list/);
  assert.match(wire, /domain_tests.*-gt 0/);
  assert.match(wire, /cargo test --workspace --locked --lib domain::tests::/);
  assert.match(
    wire,
    /\{"guard":"generated-wire-drift","status":"not_attached","owner":"Train B"\}/,
  );

  const migration = recipeBlock(justfile, "migration");
  assert.match(migration, /node scripts\/check-released-migrations\.mjs/);
  assert.match(
    migration,
    /rust-test migration_contract::v1_0_1_snapshot_upgrades_restarts_and_reads_core_state_through_application/,
  );
  assert.match(
    migration,
    /\{"guard":"released-migration-prefix","status":"passed","owner":"A4"\}/,
  );
  assert.doesNotMatch(migration, /not_attached/);

  const slice = recipeBlock(justfile, "slice");
  for (const dependency of [
    "format",
    "frontend",
    "_engineering-guards",
    "_rust-all",
    "_clippy",
    "wire",
    "migration",
  ]) {
    assert.match(slice, new RegExp(`\\b${dependency}\\b`));
  }
  const guards = recipeBlock(justfile, "_engineering-guards");
  assert.match(
    guards,
    /node --test scripts\/report-cargo-target\.test\.mjs scripts\/engineering-commands\.test\.mjs scripts\/check-released-migrations\.test\.mjs/,
  );

  const stage = recipeBlock(justfile, "stage");
  assert.match(stage, /^stage: slice/m);
  assert.match(stage, /CARGO_NET_OFFLINE=true pnpm tauri build --bundles app/);

  const targetReport = recipeBlock(justfile, "target-report");
  assert.match(targetReport, /node scripts\/report-cargo-target\.mjs/);
  assert.doesNotMatch(targetReport, /clean|delete|rewrite|rm\s+-/);

  const macContract = recipeBlock(justfile, "mac-contract");
  assert.match(macContract, /uname -s.*Darwin/);
  assert.match(macContract, /--test codex_mount_contract/);
  assert.match(
    macContract,
    /current_codex_discovers_global_and_project_directory_symlinks/,
  );
  assert.match(macContract, /--exact --ignored/);

  const release = recipeBlock(justfile, "release");
  assert.match(release, /^release: stage/m);
  assert.match(release, /"status":"manual_gates_required"/);
  assert.match(release, /tart/);
  assert.match(release, /MAC-CONTRACT/);
  assert.match(release, /manual_product_paths/);
  assert.match(release, /authorized_real_provider/);
  assert.match(release, /"provider_execution":"not_automatic"/);
  assert.match(release, /"publish":"not_automatic"/);
  assert.match(release, /exit 3/);
});

test("ordinary CI pins just and delegates only to frontend and stage", async () => {
  const ci = await readRepoFile(".github/workflows/ci.yml");
  assert.equal(
    ci.match(
      /extractions\/setup-just@53165ef7e734c5c07cb06b3c8e7b647c5aa16db3 # v4\.0\.0/g,
    )?.length,
    2,
  );
  assert.equal(ci.match(/just-version: "1\.58\.0"/g)?.length, 2);
  assert.equal(
    ci.match(/test "\$\(just --version\)" = "just 1\.58\.0"/g)?.length,
    2,
  );
  assert.equal(
    ci.match(
      /actions\/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7\.0\.1/g,
    )?.length,
    3,
  );
  assert.equal(
    ci.match(
      /actions\/setup-node@820762786026740c76f36085b0efc47a31fe5020 # v7\.0\.0/g,
    )?.length,
    2,
  );
  assert.match(
    ci,
    /frontend:\n[\s\S]*?name: Checkout\n\s+uses: actions\/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7\.0\.1\n\s+with:\n\s+fetch-tags: true[\s\S]*?run: just frontend\n\n  native:/,
  );
  assert.match(
    ci,
    /native:\n[\s\S]*?env:\n\s+CARGO_INCREMENTAL: "0"[\s\S]*?name: Checkout\n\s+uses: actions\/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7\.0\.1\n\s+with:\n\s+fetch-tags: true[\s\S]*?run: cargo fetch --locked[\s\S]*?run: just stage\n\n  secrets:/,
  );
  assert.doesNotMatch(ci, /run: just (?:release|mac-contract)/);

  const secrets = ci.slice(ci.indexOf("  secrets:"));
  assert.equal(
    createHash("sha256").update(secrets).digest("hex"),
    "77040d0c82d62559927b25cd61b641c9681d41e76f8df3a1acb0d2256c8ed5e5",
  );
});

test("release candidate freezes 1.1.0 through canonical recipes", async () => {
  const candidate = await readRepoFile(
    ".github/workflows/release-candidate.yml",
  );

  assert.equal(
    candidate.match(
      /extractions\/setup-just@53165ef7e734c5c07cb06b3c8e7b647c5aa16db3 # v4\.0\.0/g,
    )?.length,
    2,
  );
  assert.equal(candidate.match(/just-version: "1\.58\.0"/g)?.length, 2);
  assert.equal(
    candidate.match(
      /test "\$\(just --version\)" = "just 1\.58\.0"/g,
    )?.length,
    2,
  );
  assert.match(
    candidate,
    /quality:\n[\s\S]*?name: Checkout full history\n\s+uses: actions\/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7\.0\.1\n\s+with:\n\s+fetch-depth: 0[\s\S]*?run: just frontend[\s\S]*?\n\n  build:/,
  );
  assert.match(
    candidate,
    /build:\n[\s\S]*?env:\n\s+CARGO_INCREMENTAL: "0"[\s\S]*?name: Checkout exact commit\n\s+uses: actions\/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7\.0\.1\n\s+with:\n\s+fetch-tags: true[\s\S]*?run: cargo fetch --locked[\s\S]*?run: just stage/,
  );
  assert.match(
    candidate,
    /packageVersion !== "1\.1\.0" \|\| tauriVersion !== "1\.1\.0"/,
  );
  assert.equal(
    candidate.match(/SkillYard-1\.1\.0-macos-aarch64\.zip/g)?.length,
    3,
  );
  assert.match(
    candidate,
    /name: skillyard-1\.1\.0-macos-aarch64-candidate/,
  );
  assert.doesNotMatch(candidate, /\b1\.0\.1\b/);
  assert.doesNotMatch(candidate, /run: just (?:release|mac-contract)/);
  assert.doesNotMatch(
    candidate,
    /^\s+run: (?:pnpm (?:typecheck|test|tauri build)|cargo (?:fmt|clippy|test))/m,
  );
});

test("Secret scan ignores only the two verified historical false positives", async () => {
  const ignoredFingerprints = (await readRepoFile(".gitleaksignore"))
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);

  assert.deepEqual(ignoredFingerprints, [
    "b50e10bed8ce0f8b62819b6b7a0bfded133b22cb:docs/acceptance/rust-engineering/a3-command-governance.json:generic-api-key:543",
    "b50e10bed8ce0f8b62819b6b7a0bfded133b22cb:docs/acceptance/rust-engineering/a4-migration-lock.json:generic-api-key:344",
  ]);
});

test("public docs point to canonical recipes without duplicating internals", async () => {
  const readme = await readRepoFile("README.md");
  const readmeEnglish = await readRepoFile("README.en.md");
  const contributing = await readRepoFile("CONTRIBUTING.md");

  const chineseBuild = section(readme, "## 从源码构建");
  assert.match(chineseBuild, /just 1\.58\.0/);
  assert.match(chineseBuild, /`just stage`/);
  assert.doesNotMatch(chineseBuild, /\b(?:cargo|pnpm)\s+/);

  const englishBuild = section(readmeEnglish, "## Build from source");
  assert.match(englishBuild, /just 1\.58\.0/);
  assert.match(englishBuild, /`just stage`/);
  assert.doesNotMatch(englishBuild, /\b(?:cargo|pnpm)\s+/);

  assert.match(contributing, /just 1\.58\.0/);
  for (const command of [
    "just rust-test",
    "just slice",
    "just stage",
    "just release",
  ]) {
    assert.match(contributing, new RegExp(command));
  }
  assert.doesNotMatch(
    contributing,
    /cargo (?:test|fmt|clippy)|pnpm (?:typecheck|test|build|tauri build)/,
  );
});

test("target report implementation has no filesystem mutation primitive", async () => {
  const reportScript = await readRepoFile("scripts/report-cargo-target.mjs");
  assert.match(
    reportScript,
    /cargo[\s\S]*metadata[\s\S]*--locked[\s\S]*--offline[\s\S]*--no-deps/,
  );
  assert.doesNotMatch(
    reportScript,
    /\b(?:rm|unlink|rmdir|writeFile|appendFile|truncate|mkdir|rename|copyFile|createWriteStream)\b/,
  );
});
