mod blueprint;
mod contracts;
mod discovery;
mod error;
mod execution;
mod fabric;
mod fs_guard;
mod json;
mod materialize;
mod process_guard;
mod runtime_build;

use blueprint::load_blueprint;
use contracts::redact;
use error::{FabricError, FabricResult};
use fs_guard::canonical_project_root;
use json::{canonical_json, compact_json, Json};
use std::collections::BTreeMap;
use std::path::Path;

const VERSION: &str = "1.0.0-rc.4";

#[derive(Default)]
struct Arguments {
    positional: Vec<String>,
    flags: BTreeMap<String, String>,
    switches: Vec<String>,
}

impl Arguments {
    fn parse(values: &[String]) -> FabricResult<Self> {
        let mut parsed = Self::default();
        let mut index = 0usize;
        while index < values.len() {
            let value = &values[index];
            if value == "--compact" {
                parsed.switches.push(value.clone());
                index += 1;
            } else if value.starts_with("--") {
                let next = values.get(index + 1).ok_or_else(|| {
                    FabricError::new("invalid_arguments", format!("参数 {value} 缺少值。"))
                })?;
                if next.starts_with("--") {
                    return Err(FabricError::new(
                        "invalid_arguments",
                        format!("参数 {value} 缺少值。"),
                    ));
                }
                if parsed.flags.insert(value.clone(), next.clone()).is_some() {
                    return Err(FabricError::new(
                        "invalid_arguments",
                        format!("参数重复：{value}"),
                    ));
                }
                index += 2;
            } else {
                parsed.positional.push(value.clone());
                index += 1;
            }
        }
        Ok(parsed)
    }

    fn required(&self, name: &str) -> FabricResult<&str> {
        self.flags
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| FabricError::new("invalid_arguments", format!("缺少必需参数：{name}")))
    }

    fn optional(&self, name: &str) -> Option<&str> {
        self.flags.get(name).map(String::as_str)
    }

    fn compact(&self) -> bool {
        self.switches.iter().any(|value| value == "--compact")
    }

    fn reject_unknown(&self, allowed: &[&str]) -> FabricResult<()> {
        for key in self.flags.keys() {
            if !allowed.contains(&key.as_str()) {
                return Err(FabricError::new(
                    "invalid_arguments",
                    format!("未知参数：{key}"),
                ));
            }
        }
        Ok(())
    }
}

fn insert(target: &mut Json, key: &str, value: Json) {
    let _ = target.insert(key, value);
}

fn error_envelope(error: &FabricError) -> Json {
    let mut value = Json::object();
    insert(&mut value, "error", Json::from(error.code.clone()));
    insert(&mut value, "message", Json::from(error.message.clone()));
    insert(&mut value, "ok", Json::Bool(false));
    if let Some(details) = &error.details {
        insert(&mut value, "details", redact(&Json::from(details.clone())));
    }
    value
}

fn output_ok(value: &Json) -> bool {
    value
        .get_opt("ok")
        .and_then(|item| item.as_bool().ok())
        .unwrap_or(true)
}

fn fabric_command(arguments: &Arguments) -> FabricResult<Json> {
    if arguments.positional.len() != 1 {
        return Err(FabricError::new(
            "invalid_arguments",
            "fabric 只接受 check、init、repair、verify 或 connection-check。",
        ));
    }
    let action = &arguments.positional[0];
    let root = canonical_project_root(Path::new(arguments.required("--project-root")?))?;
    let blueprint = load_blueprint(&root)?;
    match action.as_str() {
        "check" => {
            arguments.reject_unknown(&["--project-root"])?;
            fabric::check(&root, &blueprint)
        }
        "init" | "repair" => {
            arguments.reject_unknown(&["--project-root", "--confirm-root"])?;
            fabric::write_action(
                &root,
                &blueprint,
                arguments.optional("--confirm-root"),
                action,
            )
        }
        "verify" => {
            arguments.reject_unknown(&["--project-root"])?;
            fabric::verify(&root, &blueprint)
        }
        "connection-check" => {
            arguments.reject_unknown(&["--project-root", "--descriptor"])?;
            discovery::check_connection(&root, Path::new(arguments.required("--descriptor")?))
        }
        _ => Err(FabricError::new(
            "invalid_arguments",
            format!("未知 fabric action：{action}"),
        )),
    }
}

fn agent_run_command(arguments: &Arguments) -> FabricResult<Json> {
    if arguments.positional.as_slice() != ["probe"] {
        return Err(FabricError::new(
            "invalid_arguments",
            "agent-run 只提供初始化、重新配置或验收用的 probe；它不是日常协作入口。",
        ));
    }
    arguments.reject_unknown(&["--project-root", "--descriptor", "--confirm-command"])?;
    let root = canonical_project_root(Path::new(arguments.required("--project-root")?))?;
    let _blueprint = load_blueprint(&root)?;
    execution::run_compatibility_probe(
        &root,
        Path::new(arguments.required("--descriptor")?),
        arguments.required("--confirm-command")?,
    )
}

fn dispatch(values: &[String]) -> FabricResult<(Json, bool)> {
    if values == ["--version"] {
        let mut value = Json::object();
        insert(&mut value, "language", Json::from("rust"));
        insert(&mut value, "ok", Json::Bool(true));
        insert(&mut value, "version", Json::from(VERSION));
        return Ok((value, false));
    }
    if values.first().map(String::as_str) == Some("internal-stamp-runtime") {
        let arguments = Arguments::parse(&values[1..])?;
        arguments.reject_unknown(&["--project-root"])?;
        if !arguments.positional.is_empty() {
            return Err(FabricError::new(
                "invalid_arguments",
                "internal-stamp-runtime 不接受位置参数。",
            ));
        }
        let root = canonical_project_root(Path::new(arguments.required("--project-root")?))?;
        return Ok((
            runtime_build::stamp_current_runtime(&root)?,
            arguments.compact(),
        ));
    }
    let executable = std::env::args()
        .next()
        .and_then(|value| {
            Path::new(&value)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or_default();
    let (mode, remaining) = if matches!(executable.as_str(), "fabric" | "agent-run") {
        (executable.as_str(), values)
    } else {
        let mode = values.first().ok_or_else(|| {
            FabricError::new("invalid_arguments", "缺少入口；需要 fabric 或 agent-run。")
        })?;
        (mode.as_str(), &values[1..])
    };
    let arguments = Arguments::parse(remaining)?;
    let value = match mode {
        "fabric" => fabric_command(&arguments)?,
        "agent-run" => agent_run_command(&arguments)?,
        _ => {
            return Err(FabricError::new(
                "invalid_arguments",
                format!("未知入口：{mode}"),
            ))
        }
    };
    Ok((value, arguments.compact()))
}

fn main() {
    let values: Vec<String> = std::env::args().skip(1).collect();
    match dispatch(&values) {
        Ok((value, compact)) => {
            if compact {
                println!("{}", compact_json(&value));
            } else {
                print!("{}", canonical_json(&value));
            }
            std::process::exit(if output_ok(&value) { 0 } else { 2 });
        }
        Err(error) => {
            print!("{}", canonical_json(&error_envelope(&error)));
            std::process::exit(error.exit_code);
        }
    }
}
