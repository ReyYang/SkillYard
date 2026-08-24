import { createHash } from "node:crypto";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { spawnSync } from "node:child_process";

const MANIFEST_PATH = "src-tauri/migrations/released-prefix.json";
const MIGRATION_DIRECTORY = "src-tauri/migrations";
const STORAGE_PATH = "src-tauri/src/storage.rs";
const RELEASED_COUNT = 26;

export class MigrationCheckError extends Error {
  constructor(code, message) {
    super(message);
    this.name = "MigrationCheckError";
    this.code = code;
  }
}

function fail(code, message) {
  throw new MigrationCheckError(code, message);
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function assertExactKeys(value, expected, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    fail("manifest_schema", `${label} must be an object`);
  }
  const actual = Object.keys(value).sort();
  const wanted = [...expected].sort();
  if (actual.length !== wanted.length || actual.some((key, index) => key !== wanted[index])) {
    fail(
      "manifest_schema",
      `${label} keys must be exactly ${wanted.join(", ")}; found ${actual.join(", ")}`,
    );
  }
}

function assertLiteral(actual, expected, label) {
  if (actual !== expected) {
    fail("manifest_schema", `${label} must be ${JSON.stringify(expected)}`);
  }
}

function assertHex(value, length, label) {
  if (typeof value !== "string" || !new RegExp(`^[0-9a-f]{${length}}$`).test(value)) {
    fail("manifest_schema", `${label} must be ${length} lowercase hexadecimal characters`);
  }
}

function git(gitRepoRoot, args, { allowFailure = false, encoding = "utf8" } = {}) {
  const result = spawnSync("git", args, {
    cwd: gitRepoRoot,
    encoding,
    maxBuffer: 16 * 1024 * 1024,
  });
  if (result.status !== 0 && !allowFailure) {
    const stderr = Buffer.isBuffer(result.stderr)
      ? result.stderr.toString("utf8")
      : result.stderr;
    fail("git_failure", `git ${args[0]} failed: ${(stderr || "unknown error").trim()}`);
  }
  return result;
}

function parseManifest(worktreeRoot) {
  let manifest;
  try {
    manifest = JSON.parse(readFileSync(path.join(worktreeRoot, MANIFEST_PATH), "utf8"));
  } catch (error) {
    fail("manifest_unreadable", `cannot read ${MANIFEST_PATH}: ${error.message}`);
  }

  assertExactKeys(
    manifest,
    [
      "schema_version",
      "protocol_id",
      "repository",
      "release",
      "canonical_bytes",
      "migrations",
      "current_only_unreleased",
    ],
    "manifest",
  );
  assertLiteral(manifest.schema_version, 1, "schema_version");
  assertLiteral(
    manifest.protocol_id,
    "skillyard-released-migration-prefix-v1",
    "protocol_id",
  );
  assertLiteral(manifest.repository, "ReyYang/SkillYard", "repository");

  assertExactKeys(
    manifest.release,
    [
      "version",
      "tag",
      "tag_object_oid",
      "tag_object_type",
      "tag_object_signature",
      "peeled_commit_oid",
      "github_release",
      "migration_count",
    ],
    "release",
  );
  assertLiteral(manifest.release.version, "1.0.1", "release.version");
  assertLiteral(manifest.release.tag, "v1.0.1", "release.tag");
  assertHex(manifest.release.tag_object_oid, 40, "release.tag_object_oid");
  assertLiteral(manifest.release.tag_object_type, "tag", "release.tag_object_type");
  assertHex(manifest.release.peeled_commit_oid, 40, "release.peeled_commit_oid");
  assertLiteral(manifest.release.migration_count, RELEASED_COUNT, "release.migration_count");

  assertExactKeys(
    manifest.release.tag_object_signature,
    ["status", "evidence"],
    "release.tag_object_signature",
  );
  assertLiteral(
    manifest.release.tag_object_signature.status,
    "unsigned",
    "release.tag_object_signature.status",
  );
  if (
    typeof manifest.release.tag_object_signature.evidence !== "string" ||
    !manifest.release.tag_object_signature.evidence.includes("GitHub commit verification is not a tag signature")
  ) {
    fail("manifest_schema", "tag signature evidence must distinguish commit verification from tag signing");
  }

  assertExactKeys(
    manifest.release.github_release,
    ["name", "url", "api_url", "draft", "prerelease", "published_at"],
    "release.github_release",
  );
  assertLiteral(manifest.release.github_release.name, "SkillYard 1.0.1", "github release name");
  assertLiteral(
    manifest.release.github_release.url,
    "https://github.com/ReyYang/SkillYard/releases/tag/v1.0.1",
    "github release URL",
  );
  assertLiteral(
    manifest.release.github_release.api_url,
    "https://api.github.com/repos/ReyYang/SkillYard/releases/tags/v1.0.1",
    "github release API URL",
  );
  assertLiteral(manifest.release.github_release.draft, false, "github release draft");
  assertLiteral(manifest.release.github_release.prerelease, false, "github release prerelease");
  assertLiteral(
    manifest.release.github_release.published_at,
    "2026-07-28T06:31:16Z",
    "github release published_at",
  );

  assertExactKeys(
    manifest.canonical_bytes,
    ["order", "line_rule", "sha256"],
    "canonical_bytes",
  );
  assertLiteral(
    manifest.canonical_bytes.order,
    "LC_ALL=C ascending repo-relative path",
    "canonical_bytes.order",
  );
  assertLiteral(
    manifest.canonical_bytes.line_rule,
    "<lowercase SHA-256><two ASCII spaces><repo-relative path><LF>",
    "canonical_bytes.line_rule",
  );
  assertHex(manifest.canonical_bytes.sha256, 64, "canonical_bytes.sha256");

  if (!Array.isArray(manifest.migrations) || manifest.migrations.length !== RELEASED_COUNT) {
    fail("manifest_schema", `migrations must contain exactly ${RELEASED_COUNT} entries`);
  }
  for (const [index, migration] of manifest.migrations.entries()) {
    assertExactKeys(
      migration,
      ["order", "version", "path", "released_in", "sha256"],
      `migrations[${index}]`,
    );
    const expected = index + 1;
    assertLiteral(migration.order, expected, `migrations[${index}].order`);
    assertLiteral(migration.version, expected, `migrations[${index}].version`);
    assertLiteral(migration.released_in, "1.0.1", `migrations[${index}].released_in`);
    assertHex(migration.sha256, 64, `migrations[${index}].sha256`);
    if (
      typeof migration.path !== "string" ||
      path.posix.normalize(migration.path) !== migration.path ||
      !new RegExp(`^${MIGRATION_DIRECTORY}/\\d{4}_[a-z0-9_]+\\.sql$`).test(migration.path)
    ) {
      fail("manifest_schema", `migrations[${index}].path is not canonical`);
    }
    const versionFromPath = Number(path.posix.basename(migration.path).slice(0, 4));
    assertLiteral(versionFromPath, expected, `migrations[${index}].path version`);
  }

  if (!Array.isArray(manifest.current_only_unreleased) || manifest.current_only_unreleased.length === 0) {
    fail("manifest_schema", "current_only_unreleased must contain the current contiguous tail after version 26");
  }
  for (const [index, migration] of manifest.current_only_unreleased.entries()) {
    assertExactKeys(migration, ["version", "path", "status"], `current_only_unreleased[${index}]`);
    const expected = RELEASED_COUNT + index + 1;
    assertLiteral(migration.version, expected, `current_only_unreleased[${index}].version`);
    assertLiteral(
      migration.status,
      "current-only/unreleased-at-v1.0.1",
      `current_only_unreleased[${index}].status`,
    );
    if (typeof migration.path !== "string" || !migration.path.startsWith(`${MIGRATION_DIRECTORY}/`)) {
      fail("manifest_schema", `current_only_unreleased[${index}].path is not canonical`);
    }
    const versionFromPath = Number(path.posix.basename(migration.path).slice(0, 4));
    assertLiteral(versionFromPath, expected, `current_only_unreleased[${index}].path version`);
  }

  const canonicalBytes = manifest.migrations
    .map((migration) => `${migration.sha256}  ${migration.path}\n`)
    .join("");
  if (sha256(canonicalBytes) !== manifest.canonical_bytes.sha256) {
    fail("aggregate_mismatch", "released migration canonical aggregate does not match manifest");
  }
  return manifest;
}

function currentMigrationEntries(worktreeRoot) {
  const directory = path.join(worktreeRoot, MIGRATION_DIRECTORY);
  let entries;
  try {
    entries = readdirSync(directory, { withFileTypes: true })
      .filter((entry) => entry.name.endsWith(".sql"))
      .map((entry) => {
        if (!entry.isFile()) {
          fail("migration_file_type", `${entry.name} must be a regular file`);
        }
        const match = /^(\d{4})_[a-z0-9_]+\.sql$/.exec(entry.name);
        if (!match) fail("migration_path_invalid", `${entry.name} is not a canonical migration path`);
        return {
          version: Number(match[1]),
          path: `${MIGRATION_DIRECTORY}/${entry.name}`,
        };
      });
  } catch (error) {
    if (error instanceof MigrationCheckError) throw error;
    fail("migration_directory_unreadable", `cannot enumerate ${MIGRATION_DIRECTORY}: ${error.message}`);
  }
  entries.sort((left, right) => left.version - right.version || left.path.localeCompare(right.path, "en"));
  const versions = new Set();
  for (const entry of entries) {
    if (versions.has(entry.version)) {
      fail("released_prefix_insertion", `migration version ${entry.version} appears more than once`);
    }
    versions.add(entry.version);
  }
  return entries;
}

function verifyCurrentFiles(worktreeRoot, manifest) {
  const current = currentMigrationEntries(worktreeRoot);
  const expected = [
    ...manifest.migrations.map(({ version, path: migrationPath }) => ({ version, path: migrationPath })),
    ...manifest.current_only_unreleased.map(({ version, path: migrationPath }) => ({
      version,
      path: migrationPath,
    })),
  ];
  for (const migration of manifest.migrations) {
    const entry = current.find((candidate) => candidate.version === migration.version);
    if (!entry) {
      fail("released_path_missing", `released migration ${migration.path} is missing`);
    }
    if (entry.path !== migration.path) {
      fail(
        "released_path_mismatch",
        `released migration ${migration.version} is ${entry.path}; expected ${migration.path}`,
      );
    }
  }
  if (current.length !== expected.length) {
    fail("current_migration_count", `manifest records ${expected.length} current migrations; found ${current.length}`);
  }
  for (const [index, entry] of current.entries()) {
    if (entry.version !== index + 1) {
      fail("released_prefix_insertion", `current migration sequence is not contiguous at order ${index + 1}`);
    }
  }
  for (const [index, entry] of current.entries()) {
    if (entry.path !== expected[index].path) {
      const code = index < RELEASED_COUNT ? "released_path_mismatch" : "current_tail_mismatch";
      fail(code, `migration ${entry.version} path is ${entry.path}; expected ${expected[index].path}`);
    }
  }
  for (const migration of manifest.migrations) {
    let bytes;
    try {
      bytes = readFileSync(path.join(worktreeRoot, migration.path));
    } catch (error) {
      fail("released_path_missing", `cannot read released migration ${migration.path}: ${error.message}`);
    }
    if (sha256(bytes) !== migration.sha256) {
      fail("checksum_mismatch", `${migration.path} differs from its released SHA-256`);
    }
  }
  return current;
}

function verifyReleaseTag(gitRepoRoot, manifest) {
  const ref = `refs/tags/${manifest.release.tag}`;
  const resolved = git(gitRepoRoot, ["rev-parse", "--verify", ref], { allowFailure: true });
  if (resolved.status !== 0) {
    fail("tag_missing", `${ref} is required and was not found`);
  }
  const tagObjectOid = resolved.stdout.trim();
  if (tagObjectOid !== manifest.release.tag_object_oid) {
    fail("tag_object_mismatch", `${ref} resolves to ${tagObjectOid}`);
  }
  const objectType = git(gitRepoRoot, ["cat-file", "-t", tagObjectOid]).stdout.trim();
  if (objectType !== "tag") fail("tag_type_mismatch", `${ref} must resolve to an annotated tag object`);

  const tagObject = git(gitRepoRoot, ["cat-file", "-p", tagObjectOid]).stdout;
  if (
    !tagObject.startsWith(`object ${manifest.release.peeled_commit_oid}\ntype commit\ntag ${manifest.release.tag}\n`) ||
    /-----BEGIN (?:PGP|SSH) SIGNATURE-----/.test(tagObject)
  ) {
    fail("tag_object_content_mismatch", "annotated tag identity or unsigned status does not match manifest");
  }
  const peeled = git(gitRepoRoot, ["rev-parse", `${ref}^{commit}`]).stdout.trim();
  if (peeled !== manifest.release.peeled_commit_oid) {
    fail("peeled_commit_mismatch", `${ref} peels to ${peeled}`);
  }

  const tagPaths = git(gitRepoRoot, [
    "ls-tree",
    "-r",
    "--name-only",
    peeled,
    "--",
    MIGRATION_DIRECTORY,
  ]).stdout.trim().split("\n").filter(Boolean).sort();
  const expectedPaths = manifest.migrations.map((migration) => migration.path);
  if (
    tagPaths.length !== expectedPaths.length ||
    tagPaths.some((migrationPath, index) => migrationPath !== expectedPaths[index])
  ) {
    fail("tag_tree_path_mismatch", "release tag migration paths do not exactly match the manifest prefix");
  }
  for (const migration of manifest.migrations) {
    const result = git(gitRepoRoot, ["show", `${peeled}:${migration.path}`], { encoding: null });
    if (sha256(result.stdout) !== migration.sha256) {
      fail("tag_tree_checksum_mismatch", `${migration.path} bytes differ in the release tag tree`);
    }
  }
  return { tagObjectOid, peeledCommitOid: peeled, paths: tagPaths };
}

function rustFiles(root) {
  const files = [];
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    const absolute = path.join(root, entry.name);
    if (entry.isDirectory()) files.push(...rustFiles(absolute));
    else if (entry.isFile() && entry.name.endsWith(".rs")) files.push(absolute);
  }
  return files;
}

function sanitizeRustSource(source) {
  const code = [];
  const strings = [];
  let index = 0;

  const retainNewlines = (start, end) => {
    for (let cursor = start; cursor < end; cursor += 1) {
      code.push(source[cursor] === "\n" || source[cursor] === "\r" ? source[cursor] : " ");
    }
  };
  const retainString = (end, content) => {
    const stringIndex = strings.push(content) - 1;
    // NUL 不能出现在有效 Rust token 中，避免字符串内容再次被 regex 当成代码。
    code.push(`\u0000${stringIndex}\u0000`);
    index = end;
  };

  while (index < source.length) {
    if (source.startsWith("//", index)) {
      const end = source.indexOf("\n", index + 2);
      const commentEnd = end === -1 ? source.length : end;
      retainNewlines(index, commentEnd);
      index = commentEnd;
      continue;
    }

    if (source.startsWith("/*", index)) {
      const start = index;
      let depth = 1;
      index += 2;
      while (index < source.length && depth > 0) {
        if (source.startsWith("/*", index)) {
          depth += 1;
          index += 2;
        } else if (source.startsWith("*/", index)) {
          depth -= 1;
          index += 2;
        } else {
          index += 1;
        }
      }
      retainNewlines(start, index);
      continue;
    }

    const raw = /^(?:b|c)?r(#{0,255})"/.exec(source.slice(index));
    if (raw) {
      const contentStart = index + raw[0].length;
      const terminator = `"${raw[1]}`;
      const terminatorStart = source.indexOf(terminator, contentStart);
      const end = terminatorStart === -1
        ? source.length
        : terminatorStart + terminator.length;
      const contentEnd = terminatorStart === -1 ? source.length : terminatorStart;
      retainString(end, source.slice(contentStart, contentEnd));
      continue;
    }

    if (source[index] === '"') {
      const contentStart = index + 1;
      let cursor = contentStart;
      while (cursor < source.length) {
        if (source[cursor] === "\\") {
          cursor += 2;
        } else if (source[cursor] === '"') {
          break;
        } else {
          cursor += 1;
        }
      }
      const end = cursor < source.length ? cursor + 1 : source.length;
      retainString(end, source.slice(contentStart, cursor));
      continue;
    }

    const character = /^'(?:\\(?:[nrt0\\'"]|x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]{1,6}\})|[^\\'\r\n])'/u.exec(
      source.slice(index),
    );
    if (character) {
      code.push(" ".repeat(character[0].length));
      index += character[0].length;
      continue;
    }

    code.push(source[index]);
    index += 1;
  }

  return { code: code.join(""), strings };
}

function verifyProductionRunner(worktreeRoot, current) {
  const sourceRoot = path.join(worktreeRoot, "src-tauri/src");
  const sourceFiles = rustFiles(sourceRoot).map((file) => ({
    file,
    source: sanitizeRustSource(readFileSync(file, "utf8")),
  }));
  const owners = sourceFiles.flatMap(({ file, source }) => {
    return [...source.code.matchAll(/\bconst\s+MIGRATIONS\s*:/g)].map(() =>
      path.relative(worktreeRoot, file).split(path.sep).join("/"),
    );
  });
  if (owners.length !== 1 || owners[0] !== STORAGE_PATH) {
    fail("production_owner_mismatch", `MIGRATIONS must have one owner at ${STORAGE_PATH}`);
  }

  const productionMigrationIncludes = sourceFiles.flatMap(({ file, source }) => {
    return [...source.code.matchAll(/include_str!\(\s*\u0000(\d+)\u0000\s*\)/g)].flatMap(
      (match) => {
        const migration = /migrations\/(.+\.sql)$/.exec(source.strings[Number(match[1])]);
        return migration
          ? [{
              owner: path.relative(worktreeRoot, file).split(path.sep).join("/"),
              path: `${MIGRATION_DIRECTORY}/${migration[1]}`,
            }]
          : [];
      },
    );
  });
  if (
    productionMigrationIncludes.length !== current.length ||
    productionMigrationIncludes.some((entry) => entry.owner !== STORAGE_PATH)
  ) {
    fail(
      "production_owner_mismatch",
      `all ${MIGRATION_DIRECTORY} include_str! calls must belong to canonical owner ${STORAGE_PATH}`,
    );
  }

  const storage = sourceFiles.find(({ file }) =>
    path.relative(worktreeRoot, file).split(path.sep).join("/") === STORAGE_PATH
  )?.source;
  if (
    !storage ||
    !/pub fn open\([\s\S]*?storage\.migrate\(\)\?;[\s\S]*?Ok\(storage\)/.test(storage.code) ||
    !/fn migrate\(&mut self\)[\s\S]*?for \(version, migration\) in MIGRATIONS/.test(storage.code)
  ) {
    fail("production_owner_mismatch", "Storage::open -> Storage::migrate -> MIGRATIONS chain is missing");
  }

  const tuples = [...storage.code.matchAll(
    /\(\s*(\d+)\s*,\s*include_str!\(\s*\u0000(\d+)\u0000\s*\)\s*,?\s*\)/g,
  )].flatMap((match) => {
    const migration = /^\.\.\/migrations\/(.+)$/.exec(storage.strings[Number(match[2])]);
    return migration
      ? [{ version: Number(match[1]), path: `${MIGRATION_DIRECTORY}/${migration[1]}` }]
      : [];
  });
  if (tuples.length !== current.length) {
    fail("runner_parse_incomplete", `parsed ${tuples.length} runner entries; expected ${current.length}`);
  }
  for (const [index, tuple] of tuples.entries()) {
    if (tuple.version !== current[index].version || tuple.path !== current[index].path) {
      fail(
        "runner_order_mismatch",
        `runner order ${index + 1} is ${tuple.version}:${tuple.path}; expected ${current[index].version}:${current[index].path}`,
      );
    }
    if (productionMigrationIncludes[index].path !== tuple.path) {
      fail(
        "production_owner_mismatch",
        `canonical migration include ${index + 1} is ${productionMigrationIncludes[index].path}; expected ${tuple.path}`,
      );
    }
  }
  return { tuples, productionMigrationIncludes };
}

export async function checkReleasedMigrations({
  worktreeRoot = process.cwd(),
  gitRepoRoot = worktreeRoot,
} = {}) {
  const manifest = parseManifest(worktreeRoot);
  const current = verifyCurrentFiles(worktreeRoot, manifest);
  const release = verifyReleaseTag(gitRepoRoot, manifest);
  const runner = verifyProductionRunner(worktreeRoot, current);
  return {
    guard: "released-migration-prefix",
    status: "passed",
    release: {
      tag: manifest.release.tag,
      tagObjectOid: release.tagObjectOid,
      tagSignature: "unsigned",
      peeledCommitOid: release.peeledCommitOid,
      migrationCount: manifest.migrations.length,
      aggregateSha256: manifest.canonical_bytes.sha256,
    },
    currentRunner: {
      migrationCount: runner.tuples.length,
      versions: runner.tuples.map((entry) => entry.version),
      currentOnlyUnreleased: manifest.current_only_unreleased.map((entry) => entry.version),
    },
    canonicalProductionOwner: {
      source: STORAGE_PATH,
      chain: "Storage::open -> Storage::migrate -> MIGRATIONS",
      migrationIncludeCount: runner.productionMigrationIncludes.length,
      proofScope: "production Rust include_str! migration references and the canonical runner tuple order",
    },
  };
}

async function main() {
  if (process.argv.length !== 2) {
    fail("unsupported_arguments", "check-released-migrations accepts no arguments or rewrite mode");
  }
  const result = await checkReleasedMigrations();
  process.stdout.write(`${JSON.stringify(result)}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    const code = error instanceof MigrationCheckError ? error.code : "unexpected_error";
    process.stderr.write(`${JSON.stringify({ guard: "released-migration-prefix", status: "failed", code, message: error.message })}\n`);
    process.exitCode = 1;
  });
}
