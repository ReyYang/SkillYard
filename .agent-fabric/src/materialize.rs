use crate::blueprint::{selected_artifacts, Artifact, Blueprint, STATE_PATH};
use crate::error::{FabricError, FabricResult};
use crate::fs_guard::{
    atomic_write, find_interrupted_temps, has_git_marker, read_regular, remove_managed_file,
    require_root_confirmation, target_for,
};
use crate::json::{canonical_json, parse_json, sha256_bytes, sha256_text, Json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct InspectRow {
    pub current_sha256: Option<String>,
    pub desired_sha256: String,
    pub kind: String,
    pub path: String,
    pub status: String,
}

#[derive(Clone, Debug)]
pub struct Inspection {
    pub rows: Vec<InspectRow>,
    pub state: Option<Json>,
}

fn managed_markers(block_id: &str) -> (String, String) {
    (
        format!("<!-- agent-fabric:block:{block_id}:start -->"),
        format!("<!-- agent-fabric:block:{block_id}:end -->"),
    )
}

fn managed_slice(text: &str, block_id: &str) -> FabricResult<Option<(usize, usize)>> {
    let (start_marker, end_marker) = managed_markers(block_id);
    let starts: Vec<_> = text.match_indices(&start_marker).collect();
    let ends: Vec<_> = text.match_indices(&end_marker).collect();
    if starts.is_empty() && ends.is_empty() {
        return Ok(None);
    }
    if starts.len() != 1 || ends.len() != 1 || ends[0].0 <= starts[0].0 {
        return Err(FabricError::new(
            "managed_block_conflict",
            format!("托管块 {block_id} 重复或残缺。"),
        ));
    }
    let mut end = ends[0].0 + end_marker.len();
    if text.as_bytes().get(end) == Some(&b'\n') {
        end += 1;
    }
    Ok(Some((starts[0].0, end)))
}

fn state_checksum_source(mut state: Json) -> FabricResult<(Json, String)> {
    let object = state
        .as_object_mut()
        .map_err(|error| FabricError::new("invalid_managed_state", error))?;
    let checksum = object
        .remove("state_sha256")
        .ok_or_else(|| FabricError::new("invalid_managed_state", "受管状态缺少 state_sha256。"))?
        .as_str()
        .map_err(|error| FabricError::new("invalid_managed_state", error))?
        .to_string();
    Ok((state, checksum))
}

pub fn read_state(root: &Path) -> FabricResult<Option<Json>> {
    match target_for(root, STATE_PATH, false) {
        Ok(path) if path.exists() => {}
        Ok(_) => return Ok(None),
        Err(error) => return Err(error),
    }
    let raw = read_regular(root, STATE_PATH)?;
    let text = String::from_utf8(raw)
        .map_err(|_| FabricError::new("invalid_managed_state", "受管状态必须是 UTF-8。"))?;
    let state =
        parse_json(&text).map_err(|error| FabricError::new("invalid_managed_state", error))?;
    let object = state
        .as_object()
        .map_err(|error| FabricError::new("invalid_managed_state", error))?;
    let expected: BTreeSet<_> = ["artifacts", "blueprint_revision", "schema", "state_sha256"]
        .into_iter()
        .collect();
    let observed: BTreeSet<_> = object.keys().map(String::as_str).collect();
    if observed != expected
        || state
            .get("schema")
            .ok()
            .and_then(|value| value.as_u64().ok())
            != Some(1)
        || state
            .get("artifacts")
            .ok()
            .and_then(|value| value.as_object().ok())
            .is_none()
    {
        return Err(FabricError::new(
            "invalid_managed_state",
            "受管状态 schema 不兼容。",
        ));
    }
    let (checksum_source, observed_checksum) = state_checksum_source(state.clone())?;
    if sha256_text(&canonical_json(&checksum_source)) != observed_checksum {
        return Err(FabricError::new(
            "invalid_managed_state",
            "受管状态完整性校验失败。",
        ));
    }
    Ok(Some(state))
}

fn previous_hash(state: Option<&Json>, path: &str) -> Option<String> {
    state
        .and_then(|value| value.get("artifacts").ok())
        .and_then(|value| value.as_object().ok())
        .and_then(|value| value.get(path))
        .and_then(|value| value.get("sha256").ok())
        .and_then(|value| value.as_str().ok())
        .map(ToString::to_string)
}

fn current_artifact_hash(
    root: &Path,
    artifact: &Artifact,
) -> FabricResult<(Option<String>, &'static str)> {
    let path = target_for(root, &artifact.path, false)?;
    if !path.exists() {
        return Ok((None, "missing"));
    }
    if artifact.kind == "file" {
        match read_regular(root, &artifact.path) {
            Ok(content) => Ok((Some(sha256_bytes(&content)), "present")),
            Err(_) => Ok((None, "conflict")),
        }
    } else {
        let content = match read_regular(root, &artifact.path) {
            Ok(value) => value,
            Err(_) => return Ok((None, "conflict")),
        };
        let text = match String::from_utf8(content) {
            Ok(value) => value,
            Err(_) => return Ok((None, "conflict")),
        };
        let block_id = artifact
            .block_id
            .as_deref()
            .ok_or_else(|| FabricError::new("invalid_block_id", "缺少 block_id。"))?;
        match managed_slice(&text, block_id) {
            Ok(Some((start, end))) => Ok((Some(sha256_text(&text[start..end])), "present")),
            Ok(None) => Ok((None, "missing")),
            Err(_) => Ok((None, "conflict")),
        }
    }
}

pub fn inspect_artifacts(root: &Path, blueprint: &Blueprint) -> FabricResult<Inspection> {
    let state = read_state(root)?;
    let mut rows = Vec::new();
    for artifact in selected_artifacts(blueprint) {
        let (current, presence) = current_artifact_hash(root, artifact)?;
        let previous = previous_hash(state.as_ref(), &artifact.path);
        let status = if presence == "conflict" {
            "conflict"
        } else if current.as_deref() == Some(&artifact.sha256) {
            "match"
        } else if current.is_none() {
            "missing"
        } else if previous.is_some() && current == previous {
            "outdated_safe"
        } else {
            "conflict"
        };
        rows.push(InspectRow {
            current_sha256: current,
            desired_sha256: artifact.sha256.clone(),
            kind: artifact.kind.clone(),
            path: artifact.path.clone(),
            status: status.to_string(),
        });
    }
    Ok(Inspection { rows, state })
}

fn row_json(row: &InspectRow) -> FabricResult<Json> {
    let mut value = Json::object();
    value
        .insert(
            "current_sha256",
            row.current_sha256
                .clone()
                .map(Json::from)
                .unwrap_or(Json::Null),
        )
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    value
        .insert("desired_sha256", Json::from(row.desired_sha256.clone()))
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    value
        .insert("kind", Json::from(row.kind.clone()))
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    value
        .insert("path", Json::from(row.path.clone()))
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    value
        .insert("status", Json::from(row.status.clone()))
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    Ok(value)
}

pub fn build_check_report(root: &Path, blueprint: &Blueprint) -> FabricResult<Json> {
    let inspection = inspect_artifacts(root, blueprint)?;
    let conflicts: Vec<_> = inspection
        .rows
        .iter()
        .filter(|row| row.status == "conflict")
        .collect();
    let writes: Vec<_> = inspection
        .rows
        .iter()
        .filter(|row| matches!(row.status.as_str(), "missing" | "outdated_safe"))
        .map(|row| Json::from(row.path.clone()))
        .collect();
    let interrupted = find_interrupted_temps(root)?;
    let overall = if !conflicts.is_empty() {
        "conflict"
    } else if !interrupted.is_empty() {
        "recoverable_interruption"
    } else if !writes.is_empty() {
        "changes_planned"
    } else {
        "clean"
    };
    let state_revision = inspection
        .state
        .as_ref()
        .and_then(|state| state.get("blueprint_revision").ok())
        .cloned()
        .unwrap_or(Json::Null);
    let mut report = Json::object();
    report
        .insert(
            "artifacts",
            Json::Array(
                inspection
                    .rows
                    .iter()
                    .map(row_json)
                    .collect::<FabricResult<Vec<_>>>()?,
            ),
        )
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    report
        .insert("blueprint_revision", Json::from(blueprint.revision.clone()))
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    report
        .insert(
            "interrupted_temps",
            Json::Array(interrupted.into_iter().map(Json::from).collect()),
        )
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    report
        .insert("managed_state_revision", state_revision)
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    report
        .insert("overall_status", Json::from(overall))
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    report
        .insert("project_root", Json::from(root.display().to_string()))
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    report
        .insert("project_root_source", Json::from("explicit"))
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    report
        .insert("requires_confirmation", Json::Bool(!writes.is_empty()))
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    report
        .insert("root_write_confirmation_required", Json::Bool(true))
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    report
        .insert("project_has_git_marker", Json::Bool(has_git_marker(root)))
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    report
        .insert("write_preview", Json::Array(writes))
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    Ok(report)
}

fn render_managed_file(root: &Path, artifact: &Artifact) -> FabricResult<Vec<u8>> {
    let target = target_for(root, &artifact.path, false)?;
    if !target.exists() {
        return Ok(artifact.payload.as_bytes().to_vec());
    }
    let existing = read_regular(root, &artifact.path)?;
    let text = String::from_utf8(existing).map_err(|_| {
        FabricError::new(
            "managed_block_conflict",
            format!("托管块目标不是 UTF-8：{}", artifact.path),
        )
    })?;
    let block_id = artifact
        .block_id
        .as_deref()
        .ok_or_else(|| FabricError::new("invalid_block_id", "缺少 block_id。"))?;
    if let Some((start, end)) = managed_slice(&text, block_id)? {
        return Ok(format!("{}{}{}", &text[..start], artifact.payload, &text[end..]).into_bytes());
    }
    let separator = if text.is_empty() || text.ends_with("\n\n") {
        ""
    } else if text.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    Ok(format!("{text}{separator}{}", artifact.payload).into_bytes())
}

#[cfg(unix)]
fn observed_mode(path: &Path) -> FabricResult<u32> {
    use std::os::unix::fs::PermissionsExt;
    Ok(fs::symlink_metadata(path)?.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn observed_mode(_path: &Path) -> FabricResult<u32> {
    Ok(0o644)
}

struct Original {
    path: String,
    content: Option<Vec<u8>>,
    mode: u32,
}

fn parent_directories(root: &Path, relative: &str) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    let mut current = root.to_path_buf();
    let components: Vec<_> = Path::new(relative).components().collect();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        current.push(component.as_os_str());
        if !current.exists() {
            directories.push(current.clone());
        }
    }
    directories
}

fn build_state(
    previous: Option<&Json>,
    selected: &[&Artifact],
    revision: &str,
) -> FabricResult<Json> {
    let mut artifacts: BTreeMap<String, Json> = previous
        .and_then(|state| state.get("artifacts").ok())
        .and_then(|value| value.as_object().ok())
        .cloned()
        .unwrap_or_default();
    for artifact in selected {
        let mut record = Json::object();
        record
            .insert("kind", Json::from(artifact.kind.clone()))
            .map_err(|error| FabricError::new("internal_json_error", error))?;
        record
            .insert("sha256", Json::from(artifact.sha256.clone()))
            .map_err(|error| FabricError::new("internal_json_error", error))?;
        artifacts.insert(artifact.path.clone(), record);
    }
    let mut state = Json::object();
    state
        .insert("artifacts", Json::Object(artifacts))
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    state
        .insert("blueprint_revision", Json::from(revision))
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    state
        .insert("schema", Json::from(1u64))
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    let checksum = sha256_text(&canonical_json(&state));
    state
        .insert("state_sha256", Json::from(checksum))
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    Ok(state)
}

fn rollback(root: &Path, originals: &[Original], created_directories: &BTreeSet<PathBuf>) {
    for original in originals.iter().rev() {
        match &original.content {
            Some(content) => {
                let _ = atomic_write(root, &original.path, content, original.mode);
            }
            None => {
                if target_for(root, &original.path, false)
                    .ok()
                    .is_some_and(|path| path.exists())
                {
                    let _ = remove_managed_file(root, &original.path);
                }
            }
        }
    }
    for directory in created_directories.iter().rev() {
        let _ = fs::remove_dir(directory);
    }
}

pub fn materialize(
    root: &Path,
    blueprint: &Blueprint,
    confirmation: Option<&str>,
) -> FabricResult<Json> {
    require_root_confirmation(root, confirmation)?;
    let inspection = inspect_artifacts(root, blueprint)?;
    let conflicts: Vec<_> = inspection
        .rows
        .iter()
        .filter(|row| row.status == "conflict")
        .collect();
    if !conflicts.is_empty() {
        return Err(FabricError::new(
            "managed_conflict",
            "发现用户修改、残缺托管块或未知内容；未写入任何文件。",
        )
        .with_details(
            conflicts
                .iter()
                .map(|row| row.path.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }
    let selected = selected_artifacts(blueprint);
    let by_path: BTreeMap<_, _> = selected
        .iter()
        .map(|artifact| (artifact.path.as_str(), *artifact))
        .collect();
    let planned: Vec<_> = inspection
        .rows
        .iter()
        .filter(|row| matches!(row.status.as_str(), "missing" | "outdated_safe"))
        .collect();
    let mut originals = Vec::new();
    let mut created_directories = BTreeSet::new();
    let mut changed = Vec::new();
    let result = (|| -> FabricResult<()> {
        for (index, row) in planned.iter().enumerate() {
            let artifact = by_path.get(row.path.as_str()).ok_or_else(|| {
                FabricError::new("internal_artifact_error", "找不到计划 artifact。")
            })?;
            created_directories.extend(parent_directories(root, &artifact.path));
            let target = target_for(root, &artifact.path, false)?;
            let original = if target.exists() {
                Original {
                    path: artifact.path.clone(),
                    content: Some(read_regular(root, &artifact.path)?),
                    mode: observed_mode(&target)?,
                }
            } else {
                Original {
                    path: artifact.path.clone(),
                    content: None,
                    mode: artifact.mode,
                }
            };
            originals.push(original);
            let content = if artifact.kind == "file" {
                artifact.payload.as_bytes().to_vec()
            } else {
                render_managed_file(root, artifact)?
            };
            atomic_write(root, &artifact.path, &content, artifact.mode)?;
            changed.push(artifact.path.clone());
            if let Ok(value) = std::env::var("AGENT_FABRIC_TEST_FAIL_AFTER") {
                if value
                    .parse::<usize>()
                    .ok()
                    .is_some_and(|limit| index + 1 >= limit)
                {
                    return Err(FabricError::new(
                        "simulated_interruption",
                        "模拟 materialization 中断。",
                    ));
                }
            }
        }
        let state = build_state(inspection.state.as_ref(), &selected, &blueprint.revision)?;
        let desired_state = canonical_json(&state);
        let state_target = target_for(root, STATE_PATH, false)?;
        if state_target.exists() && read_regular(root, STATE_PATH)? == desired_state.as_bytes() {
            return Ok(());
        }
        if state_target.exists() {
            originals.push(Original {
                path: STATE_PATH.to_string(),
                content: Some(read_regular(root, STATE_PATH)?),
                mode: observed_mode(&state_target)?,
            });
        } else {
            created_directories.extend(parent_directories(root, STATE_PATH));
            originals.push(Original {
                path: STATE_PATH.to_string(),
                content: None,
                mode: 0o644,
            });
        }
        atomic_write(root, STATE_PATH, desired_state.as_bytes(), 0o644)?;
        Ok(())
    })();
    if let Err(error) = result {
        rollback(root, &originals, &created_directories);
        return Err(error);
    }

    let post = inspect_artifacts(root, blueprint)?;
    if post.rows.iter().any(|row| row.status != "match") {
        rollback(root, &originals, &created_directories);
        return Err(FabricError::new(
            "post_write_verification_failed",
            "写入后 artifact hash 验证失败，已回滚。",
        ));
    }
    let mut output = Json::object();
    output
        .insert(
            "changed",
            Json::Array(changed.into_iter().map(Json::from).collect()),
        )
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    output
        .insert("ok", Json::Bool(true))
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    output
        .insert("project_root", Json::from(root.display().to_string()))
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    output
        .insert("verified_artifact_count", Json::from(post.rows.len()))
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_managed_markers_are_conflicts() {
        let text = "<!-- agent-fabric:block:x:start -->\nmissing end\n";
        assert_eq!(
            managed_slice(text, "x").unwrap_err().code,
            "managed_block_conflict"
        );
    }
}
