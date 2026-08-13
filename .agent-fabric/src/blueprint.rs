use crate::error::{FabricError, FabricResult};
use crate::fs_guard::{read_regular, validate_relative_path};
use crate::json::{canonical_json, parse_json, sha256_text, Json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const BLUEPRINT_FILE: &str = "AGENT-FABRIC.md";
pub const MACHINE_START: &str = "<!-- agent-fabric:machine:start -->";
pub const MACHINE_END: &str = "<!-- agent-fabric:machine:end -->";
pub const STATE_PATH: &str = ".agent-fabric/state/managed.json";

#[derive(Clone, Debug)]
pub struct Artifact {
    pub block_id: Option<String>,
    pub kind: String,
    pub mode: u32,
    pub path: String,
    pub payload: String,
    pub sha256: String,
}

#[derive(Clone, Debug)]
pub struct Blueprint {
    pub data: Json,
    pub artifacts: Vec<Artifact>,
    pub revision: String,
}

fn invalid(message: impl Into<String>) -> FabricError {
    FabricError::new("invalid_blueprint", message)
}

pub fn object(value: &Json) -> FabricResult<&BTreeMap<String, Json>> {
    value.as_object().map_err(invalid)
}

pub fn array(value: &Json) -> FabricResult<&Vec<Json>> {
    value.as_array().map_err(invalid)
}

pub fn string(value: &Json) -> FabricResult<&str> {
    value.as_str().map_err(invalid)
}

pub fn unsigned(value: &Json) -> FabricResult<u64> {
    value.as_u64().map_err(invalid)
}

pub fn field<'a>(value: &'a Json, key: &str) -> FabricResult<&'a Json> {
    value.get(key).map_err(invalid)
}

pub fn string_field<'a>(value: &'a Json, key: &str) -> FabricResult<&'a str> {
    string(field(value, key)?)
}

pub fn array_field<'a>(value: &'a Json, key: &str) -> FabricResult<&'a Vec<Json>> {
    array(field(value, key)?)
}

pub fn string_array(value: &Json) -> FabricResult<Vec<String>> {
    array(value)?
        .iter()
        .map(|item| Ok(string(item)?.to_string()))
        .collect()
}

fn extract_machine_text(markdown: &str) -> FabricResult<&str> {
    // payload 里可以出现相同文字；只有独占整行的标记具有结构语义。
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    let mut offset = 0usize;
    for line in markdown.split_inclusive('\n') {
        let value = line.strip_suffix('\n').unwrap_or(line);
        if value == MACHINE_START {
            starts.push(offset);
        } else if value == MACHINE_END {
            ends.push(offset);
        }
        offset += line.len();
    }
    if starts.len() != 1 || ends.len() != 1 || ends[0] <= starts[0] {
        return Err(FabricError::new(
            "machine_block_count",
            "Blueprint 必须且只能包含一对机器块标记。",
        ));
    }
    let region_start = starts[0] + MACHINE_START.len();
    let mut region = &markdown[region_start..ends[0]];
    region = region
        .strip_prefix('\n')
        .ok_or_else(|| FabricError::new("invalid_machine_fence", "机器块标记后必须紧跟 LF。"))?;
    if let Some(value) = region.strip_suffix('\n') {
        region = value;
    }
    let body = region
        .strip_prefix("```yaml\n")
        .and_then(|value| value.strip_suffix("```"))
        .ok_or_else(|| {
            FabricError::new(
                "invalid_machine_fence",
                "机器块必须是唯一、完整的 ```yaml fence。",
            )
        })?;
    if !body.ends_with('\n') {
        return Err(FabricError::new(
            "noncanonical_machine_block",
            "机器块必须以 LF 结尾。",
        ));
    }
    Ok(body)
}

fn exact_keys(object: &BTreeMap<String, Json>, expected: &[&str], code: &str) -> FabricResult<()> {
    let observed: BTreeSet<_> = object.keys().map(String::as_str).collect();
    let expected: BTreeSet<_> = expected.iter().copied().collect();
    if observed != expected {
        let missing: Vec<_> = expected.difference(&observed).copied().collect();
        let unknown: Vec<_> = observed.difference(&expected).copied().collect();
        return Err(FabricError::new(code, "机器恢复数据字段不一致。")
            .with_details(format!("missing={missing:?}, unknown={unknown:?}")));
    }
    Ok(())
}

fn validate_artifact(value: &Json) -> FabricResult<Artifact> {
    exact_keys(
        object(value)?,
        &["condition", "kind", "mode", "path", "payload", "sha256"],
        "invalid_artifact_fields",
    )?;
    let path = validate_relative_path(string_field(value, "path")?)?;
    if !path.starts_with(".agent-fabric/") || path.contains("/.agent-fabric/") {
        return Err(FabricError::new(
            "invalid_artifact_path",
            format!("artifact 必须直接属于项目内 .agent-fabric：{path}"),
        ));
    }
    if string_field(value, "kind")? != "file" {
        return Err(FabricError::new(
            "invalid_artifact_kind",
            "当前 Blueprint 只恢复普通文件。",
        ));
    }
    let mode = match string_field(value, "mode")? {
        "0644" => 0o644,
        "0755" => 0o755,
        value => {
            return Err(FabricError::new(
                "invalid_artifact_mode",
                format!("不支持 mode：{value}"),
            ))
        }
    };
    let condition = field(value, "condition")?;
    exact_keys(object(condition)?, &["type"], "invalid_condition")?;
    if string_field(condition, "type")? != "always" {
        return Err(FabricError::new(
            "invalid_condition",
            "Portable artifact 只能使用 always condition。",
        ));
    }
    let payload = string_field(value, "payload")?.to_string();
    if payload.contains('\r') {
        return Err(FabricError::new(
            "invalid_payload",
            format!("payload 必须只使用 LF：{path}"),
        ));
    }
    let sha256 = string_field(value, "sha256")?.to_string();
    if sha256_text(&payload) != sha256 {
        return Err(FabricError::new(
            "payload_hash_mismatch",
            format!("payload hash 不匹配：{path}"),
        ));
    }
    Ok(Artifact {
        block_id: None,
        kind: "file".to_string(),
        mode,
        path,
        payload,
        sha256,
    })
}

fn validate_machine(data: &Json) -> FabricResult<Vec<Artifact>> {
    exact_keys(
        object(data)?,
        &["artifacts", "blueprint_schema", "core", "revision_sha256"],
        "invalid_top_level_fields",
    )?;
    if unsigned(field(data, "blueprint_schema")?)? != 3 {
        return Err(FabricError::new(
            "unsupported_schema",
            "当前 Core 只支持本版 Blueprint 恢复格式。",
        ));
    }
    exact_keys(
        object(field(data, "core")?)?,
        &["language", "maintenance_optional", "rustc", "version"],
        "invalid_core_fields",
    )?;
    if string_field(field(data, "core")?, "language")? != "rust" {
        return Err(FabricError::new(
            "unsupported_core_language",
            "维护工具源码必须使用 Rust。",
        ));
    }

    let mut artifacts = Vec::new();
    let mut paths = BTreeSet::new();
    let mut folded_paths = BTreeSet::new();
    let mut previous: Option<String> = None;
    for item in array_field(data, "artifacts")? {
        let artifact = validate_artifact(item)?;
        if previous
            .as_ref()
            .is_some_and(|value| value > &artifact.path)
        {
            return Err(FabricError::new(
                "noncanonical_artifact_order",
                "artifact 必须按 path 排序。",
            ));
        }
        previous = Some(artifact.path.clone());
        if !paths.insert(artifact.path.clone())
            || !folded_paths.insert(artifact.path.to_lowercase())
        {
            return Err(FabricError::new(
                "duplicate_artifact_path",
                format!("重复 artifact path：{}", artifact.path),
            ));
        }
        artifacts.push(artifact);
    }
    if artifacts.is_empty() {
        return Err(FabricError::new(
            "missing_artifacts",
            "Blueprint 必须内嵌可恢复文件。",
        ));
    }
    Ok(artifacts)
}

pub fn load_blueprint(root: &Path) -> FabricResult<Blueprint> {
    let raw = read_regular(root, BLUEPRINT_FILE)?;
    let markdown = String::from_utf8(raw).map_err(|_| {
        FabricError::new("invalid_blueprint_encoding", "Blueprint 必须使用 UTF-8。")
    })?;
    if markdown.contains('\r') {
        return Err(FabricError::new(
            "invalid_blueprint_newlines",
            "Blueprint 必须只使用 LF。",
        ));
    }
    let machine_text = extract_machine_text(&markdown)?;
    let data = parse_json(machine_text)
        .map_err(|error| FabricError::new("invalid_machine_block", error))?;
    if canonical_json(&data) != machine_text {
        return Err(FabricError::new(
            "noncanonical_machine_block",
            "机器块必须使用规范 key 顺序、缩进与尾随 LF。",
        ));
    }
    let artifacts = validate_machine(&data)?;
    let revision = string_field(&data, "revision_sha256")?.to_string();
    let mut revision_source = data.clone();
    revision_source
        .as_object_mut()
        .map_err(invalid)?
        .remove("revision_sha256");
    let expected = sha256_text(&canonical_json(&revision_source));
    if revision != expected {
        return Err(FabricError::new(
            "blueprint_revision_mismatch",
            "Blueprint revision hash 不匹配。",
        ));
    }
    Ok(Blueprint {
        data,
        artifacts,
        revision,
    })
}

pub fn selected_artifacts(blueprint: &Blueprint) -> Vec<&Artifact> {
    blueprint.artifacts.iter().collect()
}

pub fn core_version(blueprint: &Blueprint) -> FabricResult<&str> {
    string_field(field(&blueprint.data, "core")?, "version")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_markers_must_be_unique() {
        let markdown = format!("{MACHINE_START}\n```yaml\n{{}}\n```\n{MACHINE_END}\n");
        assert_eq!(extract_machine_text(&markdown).unwrap(), "{}\n");
        let duplicate = format!("{markdown}{markdown}");
        assert_eq!(
            extract_machine_text(&duplicate).unwrap_err().code,
            "machine_block_count"
        );
    }

    #[test]
    fn nested_fabric_artifacts_are_rejected() {
        let mut value = Json::object();
        value
            .insert("condition", {
                let mut condition = Json::object();
                condition.insert("type", Json::from("always")).unwrap();
                condition
            })
            .unwrap();
        value.insert("kind", Json::from("file")).unwrap();
        value.insert("mode", Json::from("0644")).unwrap();
        value
            .insert("path", Json::from(".agent-fabric/x/.agent-fabric/y"))
            .unwrap();
        value.insert("payload", Json::from("x\n")).unwrap();
        value
            .insert("sha256", Json::from(sha256_text("x\n")))
            .unwrap();
        assert_eq!(
            validate_artifact(&value).unwrap_err().code,
            "invalid_artifact_path"
        );
    }
}
