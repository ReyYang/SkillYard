use std::{
    env, fs,
    io::Write,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
};

use crate::support::{mark_hard_exit_worker_entered, run_hard_exit_child};
use rusqlite::Connection;
use skillyard_lib::{
    ApplicationPaths, BundleUpdateBatchPlan, BundleUpdateBatchPlanItemDisposition,
    BundleUpdateBatchResult, BundleUpdateBatchResultItemStatus, BundleUpdateBatchResultStatus,
    BundleUpdateStatus, LifecycleFailpoint, MountScope, PlatformInfo, SkillYardApplication,
    SourceKind, SourceSummary, SupportedAppId, UiIntent, UiOutcome,
};
use tempfile::tempdir;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

const HARD_EXIT_WORKER: &str = "SKILLYARD_BATCH_UPDATE_HARD_EXIT_WORKER";
const HARD_EXIT_DATA_ROOT: &str = "SKILLYARD_BATCH_UPDATE_DATA_ROOT";
const HARD_EXIT_HOME: &str = "SKILLYARD_BATCH_UPDATE_HOME";
const HARD_EXIT_PLAN_ID: &str = "SKILLYARD_BATCH_UPDATE_PLAN_ID";
const HARD_EXIT_ITEM_IDS: &str = "SKILLYARD_BATCH_UPDATE_ITEM_IDS";

#[test]
fn github_available_without_an_adopted_baseline_uses_the_catalog_marker() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, _home) = ready_application(sandbox.path());
    let source_path = sandbox.path().join("sources/github-without-baseline");
    write_skill(
        &source_path.join("skills/github-without-baseline"),
        "github-without-baseline",
        "old",
    );
    let (_, source) = install_editable(&application, &source_path);
    let bundle_id = bundle_id(&source);
    let connection =
        Connection::open(data_root.join("skillyard.sqlite3")).expect("应打开真实 SQLite");
    let catalog_marker: String = connection
        .query_row(
            "SELECT catalog_marker FROM sources WHERE id = ?1",
            [&source.id],
            |row| row.get(0),
        )
        .expect("Source 必须有 Fresh Catalog marker");
    // 直接关联的 GitHub Bundle 尚无 adopted baseline 时，Inventory 已将其呈现为 Available。
    connection
        .execute(
            "UPDATE sources
             SET kind = 'github',
                 canonical_identity = 'github:fixture/no-baseline',
                 owner = 'fixture',
                 repository = 'no-baseline',
                 locator = 'https://github.com/fixture/no-baseline',
                 tracked_ref = 'main',
                 filesystem_device = NULL,
                 filesystem_inode = NULL
             WHERE id = ?1",
            [&source.id],
        )
        .expect("应建立无 adopted baseline 的 GitHub fixture");
    connection
        .execute(
            "UPDATE source_bundle_links
             SET adopted_marker = NULL,
                 update_check_status = 'not_checked',
                 update_checked_marker = NULL,
                 update_checked_at = NULL,
                 update_check_error = NULL
             WHERE bundle_id = ?1",
            [&bundle_id],
        )
        .expect("应清空 GitHub adopted baseline");
    drop(connection);

    let UiOutcome::Inventory { bundle_updates, .. } = application
        .handle(UiIntent::GetStartupState)
        .expect("应读取同一份 GitHub Available read model")
    else {
        panic!("应返回 Inventory");
    };
    assert_eq!(
        bundle_updates
            .iter()
            .find(|summary| summary.bundle_id == bundle_id)
            .expect("Inventory 应包含目标 Bundle")
            .status,
        BundleUpdateStatus::Available
    );

    let plan = create_batch_plan(&application);
    let item = plan_item(&plan, &bundle_id);
    assert_eq!(
        item.disposition,
        BundleUpdateBatchPlanItemDisposition::PreparationFailed,
        "测试没有注入 GitHub transport，但 Bundle 仍必须进入 Batch"
    );
    let persisted_marker: String = Connection::open(data_root.join("skillyard.sqlite3"))
        .expect("应重开真实 SQLite")
        .query_row(
            "SELECT target_marker
             FROM bundle_update_batch_items
             WHERE batch_id = ?1 AND bundle_id = ?2",
            [&plan.id, &bundle_id],
            |row| row.get(0),
        )
        .expect("Batch item 应保存目标 marker");
    assert_eq!(persisted_marker, catalog_marker);
}

#[test]
fn batch_plan_includes_only_explicitly_available_bundles_and_preview_is_read_only() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());

    let available = sandbox.path().join("sources/available");
    write_skill(&available.join("skills/available"), "available", "old");
    let (installed, available_source) = install_editable(&application, &available);
    let available_bundle_id = bundle_id(&available_source);
    let available_member_id = managed_member_id(&installed, "available");
    mount_codex_global(&application, &available_member_id);
    fs::write(
        available.join("skills/available/payload.txt"),
        "available-new",
    )
    .expect("应修改可更新 Source");
    check_editable(&application, &available_bundle_id);

    let up_to_date = sandbox.path().join("sources/up-to-date");
    write_skill(&up_to_date.join("skills/up-to-date"), "up-to-date", "same");
    let (_, up_to_date_source) = install_editable(&application, &up_to_date);
    check_editable(&application, &bundle_id(&up_to_date_source));

    let not_checked = sandbox.path().join("sources/not-checked");
    write_skill(
        &not_checked.join("skills/not-checked"),
        "not-checked",
        "same",
    );
    install_editable(&application, &not_checked);

    let unavailable = sandbox.path().join("sources/unavailable");
    write_skill(
        &unavailable.join("skills/unavailable"),
        "unavailable",
        "same",
    );
    let (_, unavailable_source) = install_editable(&application, &unavailable);
    let moved = sandbox.path().join("sources/unavailable-moved");
    fs::rename(&unavailable, &moved).expect("应暂时移走 Editable Local Source");
    check_editable(&application, &bundle_id(&unavailable_source));

    let archive = sandbox.path().join("downloads/manual.skill");
    write_archive(&archive, "manual", "manual");
    install_archive(&application, &archive);

    let current = current_target(&data_root, &available_bundle_id);
    let sources_before = open_sources(&application);
    let mounted_before = fs::read_to_string(home.join(".codex/skills/available/payload.txt"))
        .expect("确认前 Mount 应可读");
    let plan = create_batch_plan(&application);

    assert_eq!(plan.items.len(), 1);
    let item = &plan.items[0];
    assert_eq!(item.bundle_id, available_bundle_id);
    assert_eq!(
        item.disposition,
        BundleUpdateBatchPlanItemDisposition::Ready
    );
    let install_plan = item
        .install_plan
        .as_ref()
        .expect("Ready 必须携带 InstallPlan");
    assert_eq!(
        install_plan
            .candidates
            .iter()
            .map(|candidate| candidate.skill_name.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("available")]
    );
    assert_eq!(current_target(&data_root, &available_bundle_id), current);
    assert_eq!(open_sources(&application), sources_before);
    assert_eq!(
        fs::read_to_string(home.join(".codex/skills/available/payload.txt"))
            .expect("预览后 Mount 应可读"),
        mounted_before
    );

    let discarded = application
        .handle(UiIntent::DiscardBundleUpdateBatchPlan { plan_id: plan.id })
        .expect("应放弃 pending 批次及其 child Plan");
    assert!(matches!(discarded, UiOutcome::Inventory { .. }));
}

#[test]
fn batch_child_plan_cannot_be_confirmed_or_discarded_outside_the_coordinator() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, _data_root, _home) = ready_application(sandbox.path());
    let first = prepare_available_editable(&application, sandbox.path(), "owned-first");
    let second = prepare_available_editable(&application, sandbox.path(), "owned-second");
    let plan = create_batch_plan(&application);
    let child = plan_item(&plan, &first.bundle_id)
        .install_plan
        .as_ref()
        .expect("Ready item 必须携带 child Plan");
    let child_plan_id = child.id.clone();
    let child_candidate_ids = child
        .candidates
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect::<Vec<_>>();

    application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: child_plan_id.clone(),
            selected_candidate_ids: child_candidate_ids,
        })
        .expect_err("普通确认入口不能绕过 Batch coordinator");
    application
        .handle(UiIntent::DiscardInstallPlan {
            plan_id: child_plan_id,
        })
        .expect_err("普通放弃入口不能删除 Batch child Plan");

    let UiOutcome::BundleUpdateBatchPlan { plan: reopened } = application
        .handle(UiIntent::GetStartupState)
        .expect("越权调用后 Batch Plan 仍必须可恢复")
    else {
        panic!("应重新打开 BundleUpdateBatchPlan");
    };
    assert_eq!(reopened, plan);
    let selected_item_ids = [
        plan_item(&plan, &first.bundle_id).id.clone(),
        plan_item(&plan, &second.bundle_id).id.clone(),
    ];
    let result = confirm_batch(&application, &plan.id, &selected_item_ids);
    assert_eq!(result.status, BundleUpdateBatchResultStatus::Completed);
    assert!(
        result
            .items
            .iter()
            .all(|item| item.status == BundleUpdateBatchResultItemStatus::Succeeded)
    );
}

#[test]
fn one_preparation_failure_does_not_prevent_later_bundle_plan_creation() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, _data_root, _home) = ready_application(sandbox.path());
    let first = sandbox.path().join("sources/first");
    let second = sandbox.path().join("sources/second");
    write_skill(&first.join("first"), "first", "old");
    write_skill(&second.join("second"), "second", "old");
    let (_, first_source) = install_editable(&application, &first);
    let (_, second_source) = install_editable(&application, &second);
    fs::write(first.join("first/payload.txt"), "new").expect("应修改第一个 Source");
    fs::write(second.join("second/payload.txt"), "new").expect("应修改第二个 Source");
    check_editable(&application, &bundle_id(&first_source));
    check_editable(&application, &bundle_id(&second_source));

    let original = sandbox.path().join("sources/first-original");
    fs::rename(&first, &original).expect("应保留原登记 inode");
    write_skill(&first.join("first"), "first", "replacement-inode");
    let plan = create_batch_plan(&application);

    let first_item = plan_item(&plan, &bundle_id(&first_source));
    assert_eq!(
        first_item.disposition,
        BundleUpdateBatchPlanItemDisposition::PreparationFailed
    );
    assert!(first_item.install_plan.is_none());
    assert!(first_item.error_summary.is_some());
    let second_item = plan_item(&plan, &bundle_id(&second_source));
    assert_eq!(
        second_item.disposition,
        BundleUpdateBatchPlanItemDisposition::Ready
    );
    assert!(second_item.install_plan.is_some());
}

#[test]
fn selected_item_order_executes_complete_children_and_continues_after_ordinary_failure() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, _home) = ready_application(sandbox.path());
    let first = prepare_available_editable(&application, sandbox.path(), "first");
    let second = prepare_available_editable(&application, sandbox.path(), "second");
    let third = prepare_available_editable(&application, sandbox.path(), "third");
    write_skill(
        &third.path.join("skills/third-extra"),
        "third-extra",
        "third-extra-new",
    );
    check_editable(&application, &third.bundle_id);

    let plan = create_batch_plan(&application);
    let first_item = plan_item(&plan, &first.bundle_id);
    let second_item = plan_item(&plan, &second.bundle_id);
    let third_item = plan_item(&plan, &third.bundle_id);
    fs::write(
        data_root
            .join("bundles")
            .join(&second.bundle_id)
            .join("current/members/second/payload.txt"),
        "externally-changed",
    )
    .expect("应制造第二个 Bundle 的普通前置条件失败");

    let result = confirm_batch(
        &application,
        &plan.id,
        &[
            third_item.id.clone(),
            second_item.id.clone(),
            first_item.id.clone(),
        ],
    );
    assert_eq!(result.status, BundleUpdateBatchResultStatus::Completed);
    assert_eq!(
        result
            .items
            .iter()
            .map(|item| item.bundle_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            third.bundle_id.as_str(),
            second.bundle_id.as_str(),
            first.bundle_id.as_str(),
        ],
        "结果顺序必须保留用户确认的执行顺序"
    );
    assert_eq!(
        result.items[0].status,
        BundleUpdateBatchResultItemStatus::Succeeded
    );
    assert_eq!(
        result.items[1].status,
        BundleUpdateBatchResultItemStatus::Failed
    );
    assert_eq!(
        result.items[2].status,
        BundleUpdateBatchResultItemStatus::Succeeded
    );
    assert_eq!(
        managed_payload(&data_root, &third.bundle_id, "third"),
        "third-new"
    );
    assert_eq!(
        managed_payload(&data_root, &third.bundle_id, "third-extra"),
        "third-extra-new",
        "Batch 不能把 child Update 降级成成员选择"
    );
    assert_eq!(
        managed_payload(&data_root, &second.bundle_id, "second"),
        "externally-changed"
    );
    assert_eq!(
        managed_payload(&data_root, &first.bundle_id, "first"),
        "first-new"
    );

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), sandbox.path().join("home")),
        PlatformInfo::supported_for_test(),
    );
    let persisted = startup_batch_result(&restarted);
    assert_eq!(persisted, result);
    let acknowledged = restarted
        .handle(UiIntent::AcknowledgeBundleUpdateBatch {
            batch_id: result.id,
        })
        .expect("completed 结果应可确认并回到 Inventory");
    assert!(matches!(acknowledged, UiOutcome::Inventory { .. }));
}

#[test]
fn hard_exit_after_child_current_switch_recovers_child_and_continues_batch() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let first = prepare_available_editable(&application, sandbox.path(), "hard-first");
    let second = prepare_available_editable(&application, sandbox.path(), "hard-second");
    let plan = create_batch_plan(&application);
    let selected_item_ids = [
        plan_item(&plan, &first.bundle_id).id.clone(),
        plan_item(&plan, &second.bundle_id).id.clone(),
    ];
    drop(application);

    run_hard_exit_child("bundle_update_batch_hard_exit_worker", 91, |child| {
        child
            .env(HARD_EXIT_WORKER, "1")
            .env(HARD_EXIT_DATA_ROOT, &data_root)
            .env(HARD_EXIT_HOME, &home)
            .env(HARD_EXIT_PLAN_ID, &plan.id)
            .env(HARD_EXIT_ITEM_IDS, selected_item_ids.join(","));
    });

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    let result = startup_batch_result(&restarted);
    assert_eq!(result.status, BundleUpdateBatchResultStatus::Completed);
    assert!(
        result
            .items
            .iter()
            .all(|item| item.status == BundleUpdateBatchResultItemStatus::Succeeded)
    );
    assert_eq!(
        managed_payload(&data_root, &first.bundle_id, "hard-first"),
        "hard-first-new"
    );
    assert_eq!(
        managed_payload(&data_root, &second.bundle_id, "hard-second"),
        "hard-second-new"
    );
}

#[test]
fn blocked_child_stops_batch_and_marks_remaining_selected_item_not_executed() {
    let sandbox = tempdir().expect("应创建隔离测试目录");
    let (application, data_root, home) = ready_application(sandbox.path());
    let first = prepare_available_editable(&application, sandbox.path(), "blocked-first");
    let second = prepare_available_editable(&application, sandbox.path(), "blocked-second");
    let plan = create_batch_plan(&application);
    let remaining_plan_id = plan_item(&plan, &second.bundle_id)
        .install_plan
        .as_ref()
        .expect("Ready item 必须携带 child Plan")
        .id
        .clone();
    let selected_item_ids = vec![
        plan_item(&plan, &first.bundle_id).id.clone(),
        plan_item(&plan, &second.bundle_id).id.clone(),
    ];
    let interrupted = SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        PlatformInfo::supported_for_test(),
        LifecycleFailpoint::AfterCurrentActivated,
    );
    interrupted
        .handle(UiIntent::ConfirmBundleUpdateBatchPlan {
            plan_id: plan.id,
            selected_item_ids,
        })
        .expect_err("应在第一个 child current 切换后中断");

    let current = data_root
        .join("bundles")
        .join(&first.bundle_id)
        .join("current");
    fs::remove_file(&current).expect("应移除已切换 current");
    symlink("content-unknown", &current).expect("应制造无法自动判断的 current");

    let restarted = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home),
        PlatformInfo::supported_for_test(),
    );
    let result = startup_batch_result(&restarted);
    assert_eq!(result.status, BundleUpdateBatchResultStatus::Blocked);
    assert_eq!(
        result.items[0].status,
        BundleUpdateBatchResultItemStatus::Blocked
    );
    assert_eq!(
        result.items[1].status,
        BundleUpdateBatchResultItemStatus::NotExecuted
    );
    // Blocked 批次不会继续执行后续 child Plan，也不能把无限期保留当成恢复协议。
    let remaining_plan_count: i64 = Connection::open(data_root.join("skillyard.sqlite3"))
        .expect("应打开真实 SQLite")
        .query_row(
            "SELECT COUNT(*) FROM install_plans WHERE id = ?1",
            [&remaining_plan_id],
            |row| row.get(0),
        )
        .expect("应检查未执行 child Plan 已被清理");
    assert_eq!(remaining_plan_count, 0);
    restarted
        .handle(UiIntent::AcknowledgeBundleUpdateBatch {
            batch_id: result.id,
        })
        .expect_err("blocked 结果必须等待 Stage 10 人工恢复，不能确认删除");
}

/// 父测试只通过环境变量启动本用例；硬退出必须跳过 Rust 析构。
#[test]
fn bundle_update_batch_hard_exit_worker() {
    if env::var_os(HARD_EXIT_WORKER).is_none() {
        return;
    }
    mark_hard_exit_worker_entered("bundle_update_batch_hard_exit_worker");
    let data_root = env::var_os(HARD_EXIT_DATA_ROOT).expect("子进程必须收到数据目录");
    let home = env::var_os(HARD_EXIT_HOME).expect("子进程必须收到 home");
    let plan_id = env::var(HARD_EXIT_PLAN_ID).expect("子进程必须收到 Batch Plan ID");
    let selected_item_ids = env::var(HARD_EXIT_ITEM_IDS)
        .expect("子进程必须收到 Batch item IDs")
        .split(',')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let application = SkillYardApplication::new_with_lifecycle_failpoint(
        ApplicationPaths::for_home(data_root.into(), home.into()),
        PlatformInfo::supported_for_test(),
        LifecycleFailpoint::HardExitAfterCurrentSwitchedBeforePhase,
    );
    application
        .handle(UiIntent::ConfirmBundleUpdateBatchPlan {
            plan_id,
            selected_item_ids,
        })
        .expect("hard-exit failpoint 必须在返回前终止进程");
}

struct AvailableEditable {
    path: PathBuf,
    bundle_id: String,
}

fn prepare_available_editable(
    application: &SkillYardApplication,
    root: &Path,
    name: &str,
) -> AvailableEditable {
    let path = root.join("sources").join(name);
    write_skill(&path.join("skills").join(name), name, "old");
    let (_, source) = install_editable(application, &path);
    let bundle_id = bundle_id(&source);
    fs::write(
        path.join("skills").join(name).join("payload.txt"),
        format!("{name}-new"),
    )
    .expect("应修改 Editable Local Source");
    check_editable(application, &bundle_id);
    AvailableEditable { path, bundle_id }
}

fn ready_application(root: &Path) -> (SkillYardApplication, PathBuf, PathBuf) {
    let data_root = root.join("application-support/SkillYard");
    let home = root.join("home");
    fs::create_dir_all(&home).expect("应创建隔离 home");
    let application = SkillYardApplication::new(
        ApplicationPaths::for_home(data_root.clone(), home.clone()),
        PlatformInfo::supported_for_test(),
    );
    application
        .handle(UiIntent::StartInitialScan)
        .expect("应完成首次扫描");
    (application, data_root, home)
}

fn install_editable(application: &SkillYardApplication, path: &Path) -> (UiOutcome, SourceSummary) {
    let UiOutcome::InstallPlan { plan } = application
        .handle(UiIntent::CreateEditableLocalInstallPlan {
            input_path: path.to_string_lossy().into_owned(),
        })
        .expect("Editable Local 应生成安装 Plan")
    else {
        panic!("应返回 InstallPlan");
    };
    let installed = application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids: plan
                .candidates
                .iter()
                .map(|candidate| candidate.candidate_id.clone())
                .collect(),
        })
        .expect("应安装 Editable Local Source");
    let locator = fs::canonicalize(path)
        .expect("应规范化 Editable Local 路径")
        .to_string_lossy()
        .into_owned();
    let source = open_sources(application)
        .into_iter()
        .find(|source| source.kind == SourceKind::EditableLocal && source.locator == locator)
        .expect("应读取刚安装的 Editable Local Source");
    (installed, source)
}

fn install_archive(application: &SkillYardApplication, path: &Path) {
    let UiOutcome::InstallPlan { plan } = application
        .handle(UiIntent::CreateArchiveInstallPlan {
            input_path: path.to_string_lossy().into_owned(),
        })
        .expect("Archive 应生成安装 Plan")
    else {
        panic!("应返回 InstallPlan");
    };
    application
        .handle(UiIntent::ConfirmInstallPlan {
            plan_id: plan.id,
            selected_candidate_ids: plan
                .candidates
                .iter()
                .map(|candidate| candidate.candidate_id.clone())
                .collect(),
        })
        .expect("应安装 Archive Source");
}

fn check_editable(application: &SkillYardApplication, bundle_id: &str) {
    application
        .handle(UiIntent::CheckEditableLocalBundle {
            bundle_id: bundle_id.to_owned(),
        })
        .expect("应主动检查 Editable Local Source");
}

fn create_batch_plan(application: &SkillYardApplication) -> BundleUpdateBatchPlan {
    let UiOutcome::BundleUpdateBatchPlan { plan } = application
        .handle(UiIntent::CreateBundleUpdateBatchPlan)
        .expect("应生成“全部更新”计划")
    else {
        panic!("应返回 BundleUpdateBatchPlan");
    };
    plan
}

fn confirm_batch(
    application: &SkillYardApplication,
    plan_id: &str,
    selected_item_ids: &[String],
) -> BundleUpdateBatchResult {
    let UiOutcome::BundleUpdateBatchResult { result } = application
        .handle(UiIntent::ConfirmBundleUpdateBatchPlan {
            plan_id: plan_id.to_owned(),
            selected_item_ids: selected_item_ids.to_vec(),
        })
        .expect("应顺序执行“全部更新”")
    else {
        panic!("应返回 BundleUpdateBatchResult");
    };
    result
}

fn startup_batch_result(application: &SkillYardApplication) -> BundleUpdateBatchResult {
    let UiOutcome::BundleUpdateBatchResult { result } = application
        .handle(UiIntent::GetStartupState)
        .expect("启动应恢复并返回 Batch Result")
    else {
        panic!("应返回 BundleUpdateBatchResult");
    };
    result
}

fn plan_item<'a>(
    plan: &'a BundleUpdateBatchPlan,
    bundle_id: &str,
) -> &'a skillyard_lib::BundleUpdateBatchPlanItem {
    plan.items
        .iter()
        .find(|item| item.bundle_id == bundle_id)
        .expect("Batch Plan 应包含目标 Bundle")
}

fn open_sources(application: &SkillYardApplication) -> Vec<SourceSummary> {
    let UiOutcome::SourceDiscovery { sources, .. } = application
        .handle(UiIntent::OpenSourceDiscovery)
        .expect("应读取 Source 列表")
    else {
        panic!("应返回 SourceDiscovery");
    };
    sources
}

fn bundle_id(source: &SourceSummary) -> String {
    source
        .bundle_id
        .clone()
        .expect("已安装 Source 应关联 Bundle")
}

fn current_target(data_root: &Path, bundle_id: &str) -> PathBuf {
    fs::read_link(data_root.join("bundles").join(bundle_id).join("current"))
        .expect("应读取 Bundle current")
}

fn managed_payload(data_root: &Path, bundle_id: &str, skill_name: &str) -> String {
    fs::read_to_string(
        data_root
            .join("bundles")
            .join(bundle_id)
            .join("current/members")
            .join(skill_name)
            .join("payload.txt"),
    )
    .expect("应读取受管 Skill 内容")
}

fn managed_member_id(outcome: &UiOutcome, skill_name: &str) -> String {
    let UiOutcome::Inventory { entries, .. } = outcome else {
        panic!("应返回 Inventory");
    };
    entries
        .iter()
        .find(|entry| entry.skill_name == skill_name && entry.member_id.is_some())
        .and_then(|entry| entry.member_id.clone())
        .expect("应找到受管 Member")
}

fn mount_codex_global(application: &SkillYardApplication, member_id: &str) {
    let UiOutcome::MountPlan { plan } = application
        .handle(UiIntent::CreateMountPlan {
            member_id: member_id.to_owned(),
            app_id: SupportedAppId::Codex,
            scope: MountScope::Global,
            project_id: None,
        })
        .expect("应生成 Mount Plan")
    else {
        panic!("应返回 MountPlan");
    };
    application
        .handle(UiIntent::ConfirmMountPlan { plan_id: plan.id })
        .expect("应确认 Mount");
}

fn write_skill(path: &Path, name: &str, payload: &str) {
    fs::create_dir_all(path).expect("应创建 Skill 目录");
    fs::write(
        path.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {name} fixture\n---\n"),
    )
    .expect("应写入 SKILL.md");
    fs::write(path.join("payload.txt"), payload).expect("应写入 payload");
}

fn write_archive(path: &Path, name: &str, payload: &str) {
    fs::create_dir_all(path.parent().expect("Archive 应有父目录")).expect("应创建 Archive 目录");
    let file = fs::File::create(path).expect("应创建 Archive");
    let mut writer = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    writer
        .start_file(format!("bundle/{name}/SKILL.md"), options)
        .expect("应写入 Archive metadata");
    writer
        .write_all(format!("---\nname: {name}\ndescription: {name} fixture\n---\n").as_bytes())
        .expect("应写入 Archive metadata 内容");
    writer
        .start_file(format!("bundle/{name}/payload.txt"), options)
        .expect("应写入 Archive payload");
    writer
        .write_all(payload.as_bytes())
        .expect("应写入 Archive payload 内容");
    writer.finish().expect("应完成 Archive");
}
