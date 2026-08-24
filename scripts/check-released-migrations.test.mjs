import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { copyFile, lstat, mkdir, mkdtemp, readFile, readdir, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";

import { checkReleasedMigrations } from "./check-released-migrations.mjs";

const { test } = process.env.VITEST
  ? await import("vitest")
  : await import("node:test");
const repoRoot = process.cwd();
const trackedInputs = [
  "src-tauri/migrations/released-prefix.json",
  "src-tauri/src/storage.rs",
  ...(await readdir(path.join(repoRoot, "src-tauri/migrations")))
    .filter((name) => name.endsWith(".sql"))
    .sort()
    .map((name) => `src-tauri/migrations/${name}`),
];

async function inputFingerprint() {
  return Promise.all(
    trackedInputs.map(async (relativePath) => {
      const absolute = path.join(repoRoot, relativePath);
      const [metadata, bytes] = await Promise.all([lstat(absolute), readFile(absolute)]);
      return {
        path: relativePath,
        type: metadata.isFile() ? "file" : "other",
        mode: metadata.mode,
        size: metadata.size,
        dev: metadata.dev.toString(),
        ino: metadata.ino.toString(),
        mtimeNs: metadata.mtimeNs?.toString() ?? String(metadata.mtimeMs),
        ctimeNs: metadata.ctimeNs?.toString() ?? String(metadata.ctimeMs),
        sha256: createHash("sha256").update(bytes).digest("hex"),
      };
    }),
  );
}

async function checkerFixture({ omit, rename } = {}) {
  const root = await mkdtemp(path.join(os.tmpdir(), "skillyard-a4-checker."));
  await mkdir(path.join(root, "src-tauri/migrations"), { recursive: true });
  await mkdir(path.join(root, "src-tauri/src"), { recursive: true });
  await copyFile(
    path.join(repoRoot, "src-tauri/migrations/released-prefix.json"),
    path.join(root, "src-tauri/migrations/released-prefix.json"),
  );
  await copyFile(
    path.join(repoRoot, "src-tauri/src/storage.rs"),
    path.join(root, "src-tauri/src/storage.rs"),
  );
  const migrationNames = (await readdir(path.join(repoRoot, "src-tauri/migrations"))).filter(
    (name) => name.endsWith(".sql"),
  );
  for (const name of migrationNames) {
    if (name === omit) continue;
    await copyFile(
      path.join(repoRoot, "src-tauri/migrations", name),
      path.join(root, "src-tauri/migrations", name === rename?.from ? rename.to : name),
    );
  }
  process.stdout.write(`# retained A4 checker fixture: ${root}\n`);
  return root;
}

async function expectFixtureFailure(root, expectedCode) {
  const before = await inputFingerprint();
  await assert.rejects(
    checkReleasedMigrations({ worktreeRoot: root, gitRepoRoot: repoRoot }),
    (error) => error?.code === expectedCode,
  );
  assert.deepEqual(await inputFingerprint(), before);
}

test("v1.0.1 released migration prefix matches the recorded unsigned tag tree and current runner", async () => {
  const result = await checkReleasedMigrations({
    worktreeRoot: repoRoot,
    gitRepoRoot: repoRoot,
  });

  assert.equal(result.status, "passed");
  assert.equal(result.release.tag, "v1.0.1");
  assert.equal(result.release.migrationCount, 26);
  assert.deepEqual(result.canonicalProductionOwner, {
    source: "src-tauri/src/storage.rs",
    chain: "Storage::open -> Storage::migrate -> MIGRATIONS",
    migrationIncludeCount: 31,
    proofScope: "production Rust include_str! migration references and the canonical runner tuple order",
  });
  assert.deepEqual(result.currentRunner.versions, [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19,
    20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
  ]);
});

test("checker rejects a current migration omitted from the manifest tail", async () => {
  const root = await checkerFixture();
  const manifestPath = path.join(root, "src-tauri/migrations/released-prefix.json");
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  manifest.current_only_unreleased.pop();
  await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
  await expectFixtureFailure(root, "current_migration_count");
});

test("checker fails closed when the recorded release tag is absent", async () => {
  const gitRoot = await mkdtemp(path.join(os.tmpdir(), "skillyard-a4-no-tag."));
  const initialized = spawnSync("git", ["init", "--bare", gitRoot], {
    cwd: repoRoot,
    encoding: "utf8",
  });
  assert.equal(initialized.status, 0, initialized.stderr);
  process.stdout.write(`# retained A4 no-tag repository: ${gitRoot}\n`);
  const before = await inputFingerprint();
  await assert.rejects(
    checkReleasedMigrations({ worktreeRoot: repoRoot, gitRepoRoot: gitRoot }),
    (error) => error?.code === "tag_missing",
  );
  assert.deepEqual(await inputFingerprint(), before);
});

test("checker rejects a modified released migration without touching repository inputs", async () => {
  const root = await checkerFixture();
  const target = path.join(root, "src-tauri/migrations/0001_initial.sql");
  await writeFile(target, `${await readFile(target, "utf8")}\n-- modified negative fixture\n`);
  await expectFixtureFailure(root, "checksum_mismatch");
});

test("checker rejects a deleted released migration without touching repository inputs", async () => {
  const root = await checkerFixture({ omit: "0002_local_inventory.sql" });
  await expectFixtureFailure(root, "released_path_missing");
});

test("checker rejects a renamed released migration without touching repository inputs", async () => {
  const root = await checkerFixture({
    rename: { from: "0003_folder_install.sql", to: "0003_renamed.sql" },
  });
  await expectFixtureFailure(root, "released_path_mismatch");
});

test("checker rejects a reordered production runner without touching repository inputs", async () => {
  const root = await checkerFixture();
  const storagePath = path.join(root, "src-tauri/src/storage.rs");
  const storage = await readFile(storagePath, "utf8");
  const first = '(1, include_str!("../migrations/0001_initial.sql"))';
  const second = '(2, include_str!("../migrations/0002_local_inventory.sql"))';
  assert.ok(storage.includes(first) && storage.includes(second));
  await writeFile(
    storagePath,
    storage.replace(first, "__A4_SECOND__").replace(second, first).replace("__A4_SECOND__", second),
  );
  await expectFixtureFailure(root, "runner_order_mismatch");
});

test("checker ignores a migration tuple hidden in a Rust line comment", async () => {
  const root = await checkerFixture();
  const storagePath = path.join(root, "src-tauri/src/storage.rs");
  const storage = await readFile(storagePath, "utf8");
  const tuple = '(30, include_str!("../migrations/0030_theme_preset.sql"))';
  assert.ok(storage.includes(tuple));
  await writeFile(storagePath, storage.replace(tuple, `// ${tuple}`));
  await expectFixtureFailure(root, "production_owner_mismatch");
});

test("checker ignores a migration tuple hidden in nested Rust block comments", async () => {
  const root = await checkerFixture();
  const storagePath = path.join(root, "src-tauri/src/storage.rs");
  const storage = await readFile(storagePath, "utf8");
  const tuple = '(30, include_str!("../migrations/0030_theme_preset.sql"))';
  assert.ok(storage.includes(tuple));
  await writeFile(
    storagePath,
    storage.replace(tuple, `/* outer /* nested */ ${tuple} */`),
  );
  await expectFixtureFailure(root, "production_owner_mismatch");
});

test("checker preserves comment markers inside Rust string literals", async () => {
  const root = await checkerFixture();
  const storagePath = path.join(root, "src-tauri/src/storage.rs");
  const storage = await readFile(storagePath, "utf8");
  const owner = "const MIGRATIONS";
  assert.ok(storage.includes(owner));
  await writeFile(
    storagePath,
    storage.replace(
      owner,
      'const COMMENT_MARKERS: &str = r#"// text /* text */"#;\n\nconst MIGRATIONS',
    ),
  );
  const result = await checkReleasedMigrations({
    worktreeRoot: root,
    gitRepoRoot: repoRoot,
  });
  assert.equal(result.status, "passed");
});

test("checker rejects migration includes outside the canonical Storage owner", async () => {
  const root = await checkerFixture();
  await writeFile(
    path.join(root, "src-tauri/src/rogue.rs"),
    'const ROGUE: &str = include_str!("../migrations/0001_initial.sql");\n',
  );
  await expectFixtureFailure(root, "production_owner_mismatch");
});

test("checker rejects an insertion inside the released prefix without touching repository inputs", async () => {
  const root = await checkerFixture();
  await writeFile(
    path.join(root, "src-tauri/migrations/0015_inserted.sql"),
    "-- duplicate released-prefix version negative fixture\n",
  );
  await expectFixtureFailure(root, "released_prefix_insertion");
});

test("checker CLI rejects update, fix, rewrite, and every other argument", () => {
  for (const argument of ["--update", "--fix", "--rewrite", "--repo"]) {
    const result = spawnSync(process.execPath, ["scripts/check-released-migrations.mjs", argument], {
      cwd: repoRoot,
      encoding: "utf8",
    });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /"code":"unsupported_arguments"/);
  }
});
