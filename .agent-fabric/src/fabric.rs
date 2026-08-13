use crate::blueprint::{core_version, string_field, Blueprint};
use crate::discovery::{ensure_initial_resolution, resolution_status};
use crate::error::{FabricError, FabricResult};
use crate::json::Json;
use crate::materialize::{build_check_report, materialize};
use crate::runtime_build::{ensure_runtime, runtime_status};
use std::collections::BTreeSet;
use std::path::Path;

fn insert(target: &mut Json, key: &str, value: Json) -> FabricResult<()> {
    target
        .insert(key, value)
        .map_err(|error| FabricError::new("internal_json_error", error))
}

fn maintenance_failure(error: FabricError) -> FabricResult<Json> {
    let mut value = Json::object();
    insert(&mut value, "framework_usable", Json::Bool(true))?;
    insert(&mut value, "message", Json::from(error.message))?;
    insert(&mut value, "status", Json::from("unavailable"))?;
    Ok(value)
}

fn maintenance_summary(value: &Json) -> FabricResult<Json> {
    let status = value
        .get_opt("status")
        .and_then(|item| item.as_str().ok())
        .unwrap_or("unknown");
    let rustc_available = value
        .get_opt("rustc")
        .is_some_and(|item| !matches!(item, Json::Null));
    let mut summary = Json::object();
    insert(&mut summary, "available", Json::Bool(status == "ready"))?;
    insert(&mut summary, "framework_usable", Json::Bool(true))?;
    insert(&mut summary, "rustc_available", Json::Bool(rustc_available))?;
    insert(&mut summary, "status", Json::from(status))?;
    Ok(summary)
}

fn planned_areas(detail: &Json) -> FabricResult<Json> {
    let writes = detail
        .get("write_preview")
        .map_err(|error| FabricError::new("internal_json_error", error))?
        .as_array()
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    let mut areas = BTreeSet::new();
    for item in writes {
        let path = item
            .as_str()
            .map_err(|error| FabricError::new("internal_json_error", error))?
            .strip_prefix(".agent-fabric/")
            .unwrap_or_default();
        let area = match path.split_once('/') {
            Some((directory, _)) => format!(".agent-fabric/{directory}/"),
            None => format!(".agent-fabric/{path}"),
        };
        areas.insert(area);
    }
    Ok(Json::Array(areas.into_iter().map(Json::from).collect()))
}

fn conflicts(detail: &Json) -> FabricResult<Json> {
    let rows = detail
        .get("artifacts")
        .map_err(|error| FabricError::new("internal_json_error", error))?
        .as_array()
        .map_err(|error| FabricError::new("internal_json_error", error))?;
    let mut paths = Vec::new();
    for row in rows {
        if string_field(row, "status")? == "conflict" {
            paths.push(Json::from(string_field(row, "path")?));
        }
    }
    Ok(Json::Array(paths))
}

pub fn check(root: &Path, blueprint: &Blueprint) -> FabricResult<Json> {
    let detail = build_check_report(root, blueprint)?;
    let overall = string_field(&detail, "overall_status")?;
    let safe_to_continue = overall != "conflict";
    let mut report = Json::object();
    insert(&mut report, "conflicts", conflicts(&detail)?)?;
    insert(&mut report, "external_tools_executed", Json::Bool(false))?;
    insert(
        &mut report,
        "framework_version",
        Json::from(core_version(blueprint)?),
    )?;
    insert(&mut report, "local_configuration", resolution_status(root)?)?;
    insert(
        &mut report,
        "maintenance",
        maintenance_summary(&runtime_status(root)?)?,
    )?;
    insert(&mut report, "ok", Json::Bool(safe_to_continue))?;
    insert(&mut report, "overall_status", Json::from(overall))?;
    insert(
        &mut report,
        "project_has_git_marker",
        detail
            .get("project_has_git_marker")
            .map_err(|error| FabricError::new("internal_json_error", error))?
            .clone(),
    )?;
    insert(
        &mut report,
        "project_root",
        detail
            .get("project_root")
            .map_err(|error| FabricError::new("internal_json_error", error))?
            .clone(),
    )?;
    insert(
        &mut report,
        "requires_confirmation",
        detail
            .get("requires_confirmation")
            .map_err(|error| FabricError::new("internal_json_error", error))?
            .clone(),
    )?;
    insert(&mut report, "planned_areas", planned_areas(&detail)?)?;
    Ok(report)
}

pub fn write_action(
    root: &Path,
    blueprint: &Blueprint,
    confirmation: Option<&str>,
    action: &str,
) -> FabricResult<Json> {
    let mut result = materialize(root, blueprint, confirmation)?;
    let local_configuration = ensure_initial_resolution(root)?;
    let maintenance = match ensure_runtime(root) {
        Ok(value) => maintenance_summary(&value)?,
        Err(error) => maintenance_failure(error)?,
    };
    result
        .as_object_mut()
        .map_err(|error| FabricError::new("internal_json_error", error))?
        .remove("verified_artifact_count");
    insert(&mut result, "action", Json::from(action))?;
    insert(&mut result, "external_tools_executed", Json::Bool(false))?;
    insert(&mut result, "framework_ready", Json::Bool(true))?;
    insert(&mut result, "local_configuration", local_configuration)?;
    insert(&mut result, "maintenance", maintenance)?;
    insert(
        &mut result,
        "next_step",
        Json::from("由初始化 Agent 根据用户选择补充协作者和项目内 Skill 投影。"),
    )?;
    Ok(result)
}

pub fn verify(root: &Path, blueprint: &Blueprint) -> FabricResult<Json> {
    let report = build_check_report(root, blueprint)?;
    let framework_files = string_field(&report, "overall_status")? == "clean";
    let local_configuration = resolution_status(root)?;
    let local_configuration_ok = local_configuration
        .get_opt("ok")
        .and_then(|value| value.as_bool().ok())
        .unwrap_or(false);
    let maintenance = maintenance_summary(&runtime_status(root)?)?;
    let maintenance_status = maintenance
        .get_opt("status")
        .and_then(|value| value.as_str().ok())
        .unwrap_or("unknown");

    let mut checks = Json::object();
    insert(&mut checks, "framework_files", Json::Bool(framework_files))?;
    insert(
        &mut checks,
        "local_configuration_readable",
        Json::Bool(local_configuration_ok),
    )?;
    insert(&mut checks, "external_tools_executed", Json::Bool(false))?;
    insert(
        &mut checks,
        "maintenance_available",
        Json::Bool(maintenance_status == "ready"),
    )?;

    let mut output = Json::object();
    insert(&mut output, "checks", checks)?;
    insert(
        &mut output,
        "framework_ready",
        Json::Bool(framework_files && local_configuration_ok),
    )?;
    insert(
        &mut output,
        "framework_version",
        Json::from(core_version(blueprint)?),
    )?;
    insert(&mut output, "local_configuration", local_configuration)?;
    insert(&mut output, "maintenance", maintenance)?;
    insert(
        &mut output,
        "ok",
        Json::Bool(framework_files && local_configuration_ok),
    )?;
    insert(
        &mut output,
        "project_root",
        Json::from(root.display().to_string()),
    )?;
    Ok(output)
}
