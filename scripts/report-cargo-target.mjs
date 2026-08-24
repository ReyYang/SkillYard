#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { lstat, readdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = fileURLToPath(new URL("..", import.meta.url));
const scriptPath = fileURLToPath(import.meta.url);

function portablePath(value) {
  return value.split(path.sep).join("/");
}

function entryType(stats) {
  if (stats.isDirectory()) return "directory";
  if (stats.isFile()) return "file";
  if (stats.isSymbolicLink()) return "symlink";
  return "other";
}

function modeString(stats) {
  return (stats.mode & 0o7777n).toString(8).padStart(4, "0");
}

function inodeIdentity(stats) {
  return `${stats.dev}:${stats.ino}`;
}

function allocatedBytes(stats) {
  return Number(stats.blocks * 512n);
}

async function walkTree(root, visitor) {
  async function walk(absolutePath, relativePath) {
    const stats = await lstat(absolutePath, { bigint: true });
    await visitor(absolutePath, portablePath(relativePath), stats);

    if (stats.isDirectory()) {
      for (const name of (await readdir(absolutePath)).sort()) {
        await walk(
          path.join(absolutePath, name),
          relativePath ? path.join(relativePath, name) : name,
        );
      }
    }
  }

  await walk(root, "");
}

async function fingerprintTree(root) {
  const rows = [];
  await walkTree(root, (_absolutePath, relativePath, stats) => {
    rows.push(
      [
        relativePath || ".",
        entryType(stats),
        modeString(stats),
        stats.size.toString(),
        stats.dev.toString(),
        stats.ino.toString(),
        stats.mtimeNs.toString(),
        stats.ctimeNs.toString(),
      ].join("\t"),
    );
  });
  const material = `${rows.join("\n")}\n`;
  return {
    entries: rows.length,
    sha256: createHash("sha256").update(material).digest("hex"),
  };
}

async function collectRegularFiles(root, targetRoot) {
  const entries = [];
  await walkTree(root, (absolutePath, _relativePath, stats) => {
    if (!stats.isFile()) return;
    entries.push({
      absolutePath,
      relativePath: portablePath(path.relative(targetRoot, absolutePath)),
      inode: inodeIdentity(stats),
      apparentBytes: Number(stats.size),
      allocatedBytes: allocatedBytes(stats),
      mode: modeString(stats),
      executable: (stats.mode & 0o111n) !== 0n,
    });
  });
  return entries.sort((left, right) =>
    left.relativePath.localeCompare(right.relativePath),
  );
}

function deduplicateByInode(entries) {
  const unique = new Map();
  for (const entry of entries) {
    if (!unique.has(entry.inode)) unique.set(entry.inode, entry);
  }
  return [...unique.values()];
}

async function scanPartition(targetRoot, relativePath) {
  const absolutePath = relativePath
    ? path.join(targetRoot, relativePath)
    : targetRoot;
  let stats;
  try {
    stats = await lstat(absolutePath, { bigint: true });
  } catch (error) {
    if (error?.code === "ENOENT") {
      return {
        exists: false,
        raw_file_entries: 0,
        unique_files: 0,
        apparent_bytes: 0,
        allocated_bytes: 0,
        files: [],
      };
    }
    throw error;
  }
  if (!stats.isDirectory()) {
    throw new Error(`${relativePath || "target"} is not a directory`);
  }

  const files = await collectRegularFiles(absolutePath, targetRoot);
  const uniqueFiles = deduplicateByInode(files);
  return {
    exists: true,
    raw_file_entries: files.length,
    unique_files: uniqueFiles.length,
    apparent_bytes: uniqueFiles.reduce(
      (total, entry) => total + entry.apparentBytes,
      0,
    ),
    allocated_bytes: uniqueFiles.reduce(
      (total, entry) => total + entry.allocatedBytes,
      0,
    ),
    files: uniqueFiles,
  };
}

function publicPartition(partition) {
  const { files: _files, ...values } = partition;
  return values;
}

function cargoMetadata() {
  const result = spawnSync(
    "cargo",
    [
      "metadata",
      "--locked",
      "--offline",
      "--no-deps",
      "--format-version",
      "1",
    ],
    {
      cwd: repoRoot,
      encoding: "utf8",
      env: { ...process.env, CARGO_NET_OFFLINE: "true" },
    },
  );
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(
      `cargo metadata failed with exit ${result.status}: ${result.stderr.trim()}`,
    );
  }
  return JSON.parse(result.stdout);
}

function integrationTargets(metadata) {
  const byName = new Map();
  for (const packageMetadata of metadata.packages) {
    for (const target of packageMetadata.targets) {
      if (!target.kind.includes("test")) continue;
      if (byName.has(target.name)) {
        throw new Error(`duplicate integration target name: ${target.name}`);
      }
      byName.set(target.name, target);
    }
  }
  return [...byName.keys()].sort();
}

function integrationExecutableReport(expectedTargets, depsFiles) {
  const candidates = [];
  for (const target of expectedTargets) {
    const prefix = `${target}-`;
    for (const file of depsFiles) {
      const name = path.basename(file.relativePath);
      const executable = file.executable || name.endsWith(".exe");
      if (!name.startsWith(prefix) || !executable) continue;
      candidates.push({
        target,
        relative_path: file.relativePath,
        apparent_bytes: file.apparentBytes,
        allocated_bytes: file.allocatedBytes,
        executable: true,
      });
    }
  }
  candidates.sort(
    (left, right) =>
      left.target.localeCompare(right.target) ||
      left.relative_path.localeCompare(right.relative_path),
  );

  const targetsWithCandidates = new Set(candidates.map((entry) => entry.target));
  return {
    expected_targets: expectedTargets,
    expected_target_count: expectedTargets.length,
    candidate_count: candidates.length,
    missing_targets: expectedTargets.filter(
      (target) => !targetsWithCandidates.has(target),
    ),
    candidates,
  };
}

function largestFiles(files) {
  return [...files]
    .sort(
      (left, right) =>
        right.apparentBytes - left.apparentBytes ||
        left.relativePath.localeCompare(right.relativePath),
    )
    .slice(0, 10)
    .map((entry) => ({
      relative_path: entry.relativePath,
      apparent_bytes: entry.apparentBytes,
      allocated_bytes: entry.allocatedBytes,
      mode: entry.mode,
      executable: entry.executable,
    }));
}

function parseOutputMode(arguments_) {
  if (arguments_.length === 0) return "human";
  if (arguments_.length === 1 && arguments_[0] === "--json") return "json";
  throw new Error(`unsupported argument: ${arguments_.join(" ")}`);
}

async function resolveTargetDirectory() {
  const targetDirectory = path.resolve(
    process.env.CARGO_TARGET_DIR || path.join(repoRoot, "target"),
  );
  let stats;
  try {
    stats = await lstat(targetDirectory, { bigint: true });
  } catch (error) {
    if (error?.code === "ENOENT") {
      throw new Error(`Cargo target directory does not exist: ${targetDirectory}`);
    }
    throw error;
  }
  if (!stats.isDirectory()) {
    throw new Error(`Cargo target path is not a directory: ${targetDirectory}`);
  }
  return targetDirectory;
}

async function buildReport() {
  const targetDirectory = await resolveTargetDirectory();
  const fingerprintBefore = await fingerprintTree(targetDirectory);
  const [target, debug, debugDeps, debugIncremental, release] =
    await Promise.all([
      scanPartition(targetDirectory, ""),
      scanPartition(targetDirectory, "debug"),
      scanPartition(targetDirectory, path.join("debug", "deps")),
      scanPartition(targetDirectory, path.join("debug", "incremental")),
      scanPartition(targetDirectory, "release"),
    ]);
  const metadata = cargoMetadata();
  const expectedTargets = integrationTargets(metadata);
  const integrationExecutables = integrationExecutableReport(
    expectedTargets,
    debugDeps.files,
  );
  const fingerprintAfter = await fingerprintTree(targetDirectory);

  return {
    schema_version: 1,
    target_directory: targetDirectory,
    target_directory_source: process.env.CARGO_TARGET_DIR
      ? "CARGO_TARGET_DIR"
      : "workspace_default",
    metadata_command:
      "cargo metadata --locked --offline --no-deps --format-version 1",
    accounting: {
      inode_identity: "(st_dev,st_ino)",
      apparent_bytes: "sum st_size once per unique inode",
      allocated_bytes: "sum st_blocks * 512 once per unique inode",
      partitions_overlap: true,
      partitions_must_not_be_summed: true,
    },
    target: publicPartition(target),
    partitions: {
      debug: publicPartition(debug),
      debug_deps: publicPartition(debugDeps),
      debug_incremental: publicPartition(debugIncremental),
      release: publicPartition(release),
    },
    integration_test_executables: integrationExecutables,
    largest_unique_files: largestFiles(target.files),
    mutation: {
      fingerprint_fields: [
        "path",
        "type",
        "mode",
        "size",
        "device",
        "inode",
        "mtime_ns",
        "ctime_ns",
      ],
      fingerprint_before: fingerprintBefore,
      fingerprint_after: fingerprintAfter,
      changed:
        fingerprintBefore.entries !== fingerprintAfter.entries ||
        fingerprintBefore.sha256 !== fingerprintAfter.sha256,
    },
  };
}

function printHuman(report) {
  console.log(`Cargo target: ${report.target_directory}`);
  console.log(
    `Target: ${report.target.unique_files} unique files, ${report.target.apparent_bytes} apparent bytes, ${report.target.allocated_bytes} allocated bytes`,
  );
  for (const [name, partition] of Object.entries(report.partitions)) {
    console.log(
      `${name}: ${partition.exists ? "present" : "absent"}, ${partition.unique_files} unique files, ${partition.apparent_bytes} apparent bytes, ${partition.allocated_bytes} allocated bytes`,
    );
  }
  console.log(
    `Integration targets: ${report.integration_test_executables.expected_targets.join(", ") || "none"}`,
  );
  for (const candidate of report.integration_test_executables.candidates) {
    console.log(
      `  ${candidate.target}: ${candidate.relative_path} (${candidate.apparent_bytes} bytes)`,
    );
  }
  console.log("Largest unique files:");
  for (const file of report.largest_unique_files) {
    console.log(`  ${file.relative_path}: ${file.apparent_bytes} bytes`);
  }
  console.log(
    `Mutation fingerprint: ${report.mutation.fingerprint_before.sha256} -> ${report.mutation.fingerprint_after.sha256}; changed=${report.mutation.changed}`,
  );
  console.log("Partitions overlap and must not be summed.");
}

async function main() {
  const outputMode = parseOutputMode(process.argv.slice(2));
  const report = await buildReport();
  if (report.mutation.changed) {
    throw new Error("Cargo target changed while the report was being generated");
  }
  if (outputMode === "json") {
    console.log(JSON.stringify(report, null, 2));
  } else {
    printHuman(report);
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === scriptPath) {
  main().catch((error) => {
    console.error(`target-report: ${error.message}`);
    process.exitCode = 2;
  });
}
