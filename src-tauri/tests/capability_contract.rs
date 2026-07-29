use std::fs;

use serde_json::Value;

#[test]
fn frontend_has_only_the_minimum_tauri_capability() {
    let crate_root = env!("CARGO_MANIFEST_DIR");
    let capability_path = format!("{crate_root}/capabilities/main.json");
    let capability: Value = serde_json::from_str(
        &fs::read_to_string(capability_path).expect("应读取 Tauri capability"),
    )
    .expect("capability 应为合法 JSON");

    assert_eq!(
        capability["permissions"],
        serde_json::json!(["core:default"])
    );
}

#[test]
fn application_does_not_depend_on_generic_fs_sql_or_shell_plugins() {
    let crate_root = env!("CARGO_MANIFEST_DIR");
    let rust_manifest =
        fs::read_to_string(format!("{crate_root}/Cargo.toml")).expect("应读取 Rust manifest");
    let web_manifest =
        fs::read_to_string(format!("{crate_root}/../package.json")).expect("应读取 Web manifest");
    let manifests = format!("{rust_manifest}\n{web_manifest}");

    for forbidden in [
        "tauri-plugin-fs",
        "tauri-plugin-sql",
        "tauri-plugin-shell",
        "@tauri-apps/plugin-fs",
        "@tauri-apps/plugin-sql",
        "@tauri-apps/plugin-shell",
    ] {
        assert!(
            !manifests.contains(forbidden),
            "不应依赖通用高权限插件：{forbidden}"
        );
    }
}

#[test]
fn application_manifests_publish_one_1_1_0_version() {
    let crate_root = env!("CARGO_MANIFEST_DIR");
    let web_manifest: Value = serde_json::from_str(
        &fs::read_to_string(format!("{crate_root}/../package.json")).expect("应读取 Web manifest"),
    )
    .expect("Web manifest 应为合法 JSON");
    let tauri_config: Value = serde_json::from_str(
        &fs::read_to_string(format!("{crate_root}/tauri.conf.json")).expect("应读取 Tauri config"),
    )
    .expect("Tauri config 应为合法 JSON");

    // 用户看到的应用版本必须与前后端包版本一致，避免构建出错误版本的安装包。
    assert_eq!(env!("CARGO_PKG_VERSION"), "1.1.0");
    assert_eq!(web_manifest["version"], "1.1.0");
    assert_eq!(tauri_config["version"], "1.1.0");
}
