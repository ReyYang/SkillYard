import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  access,
  chmod,
  link,
  lstat,
  mkdir,
  mkdtemp,
  open,
  readdir,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";

const { test } = process.env.VITEST
  ? await import("vitest")
  : await import("node:test");
const repoRoot = process.cwd();
const reportScript = path.join(repoRoot, "scripts", "report-cargo-target.mjs");

async function fingerprintTree(root) {
  const rows = [];

  async function walk(absolutePath, relativePath) {
    const stats = await lstat(absolutePath, { bigint: true });
    const type = stats.isDirectory()
      ? "directory"
      : stats.isFile()
        ? "file"
        : stats.isSymbolicLink()
          ? "symlink"
          : "other";

    rows.push(
      [
        relativePath || ".",
        type,
        (stats.mode & 0o7777n).toString(8),
        stats.size.toString(),
        stats.dev.toString(),
        stats.ino.toString(),
        stats.mtimeNs.toString(),
        stats.ctimeNs.toString(),
      ].join("\t"),
    );

    if (stats.isDirectory()) {
      for (const name of (await readdir(absolutePath)).sort()) {
        await walk(
          path.join(absolutePath, name),
          relativePath ? `${relativePath}/${name}` : name,
        );
      }
    }
  }

  await walk(root, "");
  const material = `${rows.join("\n")}\n`;
  return {
    entries: rows.length,
    sha256: createHash("sha256").update(material).digest("hex"),
  };
}

async function createTargetFixture() {
  const targetDir = await mkdtemp(
    path.join(tmpdir(), "skillyard-target-report-fixture-"),
  );
  const depsDir = path.join(targetDir, "debug", "deps");
  const incrementalDir = path.join(targetDir, "debug", "incremental");
  const releaseDir = path.join(targetDir, "release");
  await mkdir(depsDir, { recursive: true });
  await mkdir(incrementalDir, { recursive: true });
  await mkdir(releaseDir, { recursive: true });

  const allExecutable = path.join(depsDir, "all-fixturehash");
  const contractExecutable = path.join(
    depsDir,
    "codex_mount_contract-fixturehash",
  );
  await writeFile(allExecutable, "all integration fixture\n");
  await writeFile(contractExecutable, "contract integration fixture\n");
  await chmod(allExecutable, 0o755);
  await chmod(contractExecutable, 0o755);

  const shared = path.join(depsDir, "shared-hardlink.bin");
  await writeFile(shared, "one physical inode\n");
  await link(shared, path.join(incrementalDir, "shared-hardlink-alias.bin"));
  await writeFile(path.join(depsDir, "library.rlib"), Buffer.alloc(16 * 1024, 7));

  const sparsePath = path.join(incrementalDir, "sparse.bin");
  const sparse = await open(sparsePath, "w");
  try {
    await sparse.truncate(128 * 1024 * 1024);
    await sparse.write(Buffer.from([1]), 0, 1, 0);
  } finally {
    await sparse.close();
  }

  const releaseExecutable = path.join(releaseDir, "skillyard");
  await writeFile(releaseExecutable, "release fixture\n");
  await chmod(releaseExecutable, 0o755);

  return targetDir;
}

test("target report deduplicates inodes and leaves the target byte-for-byte untouched", async () => {
  const targetDir = await createTargetFixture();
  const before = await fingerprintTree(targetDir);
  const result = spawnSync(process.execPath, [reportScript, "--json"], {
    cwd: repoRoot,
    encoding: "utf8",
    env: {
      ...process.env,
      CARGO_NET_OFFLINE: "true",
      CARGO_TARGET_DIR: targetDir,
    },
  });

  assert.equal(result.status, 0, result.stderr || result.stdout);
  const report = JSON.parse(result.stdout);

  assert.equal(report.schema_version, 1);
  assert.equal(report.accounting.inode_identity, "(st_dev,st_ino)");
  assert.equal(report.accounting.partitions_overlap, true);
  assert.equal(report.accounting.partitions_must_not_be_summed, true);
  assert.equal(report.target.exists, true);
  assert.equal(report.target.raw_file_entries, 7);
  assert.equal(report.target.unique_files, 6);
  assert.ok(report.target.apparent_bytes > report.target.allocated_bytes);

  assert.equal(report.partitions.debug.exists, true);
  assert.equal(report.partitions.debug_deps.exists, true);
  assert.equal(report.partitions.debug_incremental.exists, true);
  assert.equal(report.partitions.release.exists, true);
  assert.ok(
    report.partitions.debug_deps.raw_file_entries +
      report.partitions.debug_incremental.raw_file_entries >
      report.partitions.debug.unique_files,
  );

  assert.deepEqual(report.integration_test_executables.expected_targets, [
    "all",
    "codex_mount_contract",
  ]);
  assert.deepEqual(
    report.integration_test_executables.candidates.map((entry) => entry.target),
    ["all", "codex_mount_contract"],
  );
  assert.ok(
    report.integration_test_executables.candidates.every(
      (entry) => entry.executable && entry.apparent_bytes > 0,
    ),
  );
  assert.equal(
    report.largest_unique_files[0].relative_path,
    "debug/incremental/sparse.bin",
  );

  assert.equal(report.mutation.changed, false);
  assert.equal(
    report.mutation.fingerprint_before.sha256,
    report.mutation.fingerprint_after.sha256,
  );
  assert.deepEqual(await fingerprintTree(targetDir), before);

  for (const rejectedArgument of ["clean", "delete", "rewrite", "--unknown"]) {
    const rejected = spawnSync(process.execPath, [reportScript, rejectedArgument], {
      cwd: repoRoot,
      encoding: "utf8",
      env: { ...process.env, CARGO_TARGET_DIR: targetDir },
    });
    assert.notEqual(rejected.status, 0, rejectedArgument);
  }

  const missingTarget = path.join(targetDir, "does-not-exist");
  const missing = spawnSync(process.execPath, [reportScript, "--json"], {
    cwd: repoRoot,
    encoding: "utf8",
    env: { ...process.env, CARGO_TARGET_DIR: missingTarget },
  });
  assert.notEqual(missing.status, 0);
  await assert.rejects(access(missingTarget));
});
