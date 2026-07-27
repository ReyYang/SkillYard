use std::collections::BTreeSet;

use thiserror::Error;
use uuid::Uuid;

use crate::{
    domain::{
        BundleUpdateBatchPlan, BundleUpdateBatchPlanItem, BundleUpdateBatchPlanItemDisposition,
        BundleUpdateBatchResult, BundleUpdateBatchResultItem, BundleUpdateBatchResultItemStatus,
        BundleUpdateBatchResultStatus, BundleUpdateImpact, InstallCandidate, InstallInputKind,
        InstallMode, InstallPlan, SourceKind, UiOutcome,
    },
    github_source::SourceTransport,
    lifecycle::{
        LifecycleError, LifecycleFailpoint, acquire_lifecycle_lock,
        confirm_bundle_update_batch_child_install, create_bundle_update_plan,
        discard_bundle_update_batch_child_plan, discard_install_plan,
    },
    paths::ApplicationPaths,
    storage::{
        BundleUpdateBatchChildOwner, NewBundleUpdateBatchItem, Storage, StorageError,
        StoredBundleUpdateBatch, StoredBundleUpdateBatchItem, StoredInstallPlan,
    },
};

const BATCH_PLAN_TTL_MILLIS: i64 = 30 * 60 * 1_000;

#[derive(Debug, Error)]
pub enum BundleUpdateBatchError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
    #[error("当前没有已检查且可更新的 Bundle")]
    NoEligibleBundles,
}

struct PreparedBatchItem {
    id: String,
    source_id: String,
    bundle_id: String,
    display_name: String,
    install_plan_id: Option<String>,
    target_marker: String,
    status: &'static str,
    error: Option<String>,
}

/// 每个 eligible Bundle 独立准备普通 Update Plan；一个准备失败不能截断后续 Bundle。
pub fn create_bundle_update_batch_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    transport: Option<&dyn SourceTransport>,
    now: i64,
) -> Result<BundleUpdateBatchPlan, BundleUpdateBatchError> {
    if storage.read_open_bundle_update_batch()?.is_some() {
        return Err(StorageError::BundleUpdateBatchAlreadyOpen.into());
    }
    let eligible = storage.read_eligible_bundle_updates()?;
    if eligible.is_empty() {
        return Err(BundleUpdateBatchError::NoEligibleBundles);
    }

    let batch_id = Uuid::new_v4().to_string();
    let mut items = Vec::with_capacity(eligible.len());
    for eligible_item in eligible {
        let item_id = Uuid::new_v4().to_string();
        let lifecycle_lock = acquire_lifecycle_lock(paths)?;
        let prepared = create_bundle_update_plan(
            paths,
            &lifecycle_lock,
            storage,
            transport,
            &eligible_item.bundle_id,
            now,
        );
        drop(lifecycle_lock);
        match prepared {
            Ok(plan) => {
                let stored = storage.read_install_plan(&plan.id)?;
                let target_marker = stored
                    .source_marker
                    .clone()
                    .ok_or(StorageError::InvalidInstallPlan)?;
                items.push(PreparedBatchItem {
                    id: item_id,
                    source_id: eligible_item.source_id,
                    bundle_id: eligible_item.bundle_id,
                    display_name: eligible_item.bundle_display_name,
                    install_plan_id: Some(plan.id),
                    target_marker,
                    status: "ready",
                    error: None,
                });
            }
            Err(error) => items.push(PreparedBatchItem {
                id: item_id,
                source_id: eligible_item.source_id,
                bundle_id: eligible_item.bundle_id,
                display_name: eligible_item.bundle_display_name,
                install_plan_id: None,
                target_marker: eligible_item.target_marker,
                status: "preparation_failed",
                error: Some(error.to_string()),
            }),
        }
    }

    let new_items = items
        .iter()
        .map(|item| NewBundleUpdateBatchItem {
            id: &item.id,
            source_id: &item.source_id,
            bundle_id: &item.bundle_id,
            display_name: &item.display_name,
            install_plan_id: item.install_plan_id.as_deref(),
            target_marker: &item.target_marker,
            status: item.status,
            error: item.error.as_deref(),
        })
        .collect::<Vec<_>>();
    let expires_at = now.saturating_add(BATCH_PLAN_TTL_MILLIS);
    if let Err(error) = storage.save_bundle_update_batch(&batch_id, &new_items, now, expires_at) {
        cleanup_prepared_plans(paths, storage, &items)?;
        return Err(error.into());
    }
    let stored = storage.read_bundle_update_batch(&batch_id)?;
    render_batch_plan(paths, storage, &stored)
}

/// 确认只接收 batch item ID；每个 child 的完整候选集合由持久 Plan 自己派生。
pub fn confirm_bundle_update_batch_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    batch_id: &str,
    selected_item_ids: &[String],
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<BundleUpdateBatchResult, BundleUpdateBatchError> {
    let existing = storage.read_bundle_update_batch(batch_id)?;
    if matches!(existing.status.as_str(), "completed" | "blocked") {
        return render_batch_result(&existing);
    }
    if existing.status == "pending" {
        storage.begin_bundle_update_batch(batch_id, selected_item_ids, now)?;
    } else if existing.status != "running" {
        return Err(StorageError::InvalidBundleUpdateBatch.into());
    }
    reconcile_running_bundle_update_batch(paths, storage, batch_id, now, failpoint)?;
    let finished = storage.read_bundle_update_batch(batch_id)?;
    render_batch_result(&finished)
}

/// 放弃先删除协调记录，再清理 child 快照；进程中断时孤立 Plan 仍由既有 TTL 恢复器处理。
pub fn discard_bundle_update_batch_plan(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    batch_id: &str,
) -> Result<(), BundleUpdateBatchError> {
    let batch = storage.read_bundle_update_batch(batch_id)?;
    if batch.status != "pending" {
        return Err(StorageError::BundleUpdateBatchConsumed.into());
    }
    let plan_ids = batch
        .items
        .iter()
        .filter_map(|item| item.install_plan_id.clone())
        .collect::<Vec<_>>();
    storage.delete_pending_bundle_update_batch(batch_id)?;
    for plan_id in plan_ids {
        discard_install_plan(paths, storage, &plan_id)?;
    }
    Ok(())
}

pub fn acknowledge_bundle_update_batch(
    storage: &mut Storage,
    batch_id: &str,
) -> Result<(), BundleUpdateBatchError> {
    storage.acknowledge_bundle_update_batch(batch_id)?;
    Ok(())
}

/// Install child 恢复必须先运行；这里再用 adopted marker、Plan 与 blocked 状态归并 coordinator。
pub fn recover_running_bundle_update_batch(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), BundleUpdateBatchError> {
    let Some(batch) = storage.read_open_bundle_update_batch()? else {
        return Ok(());
    };
    if batch.status == "running" {
        reconcile_running_bundle_update_batch(paths, storage, &batch.id, now, failpoint)?;
    } else if batch.status == "blocked" {
        cleanup_not_executed_plans(paths, storage, &batch)?;
    }
    Ok(())
}

pub fn read_open_bundle_update_batch_outcome(
    paths: &ApplicationPaths,
    storage: &Storage,
) -> Result<Option<UiOutcome>, BundleUpdateBatchError> {
    let Some(batch) = storage.read_open_bundle_update_batch()? else {
        return Ok(None);
    };
    match batch.status.as_str() {
        "pending" => Ok(Some(UiOutcome::BundleUpdateBatchPlan {
            plan: render_batch_plan(paths, storage, &batch)?,
        })),
        "completed" | "blocked" => Ok(Some(UiOutcome::BundleUpdateBatchResult {
            result: render_batch_result(&batch)?,
        })),
        "running" => Err(StorageError::InvalidBundleUpdateBatch.into()),
        _ => Err(StorageError::InvalidBundleUpdateBatch.into()),
    }
}

fn reconcile_running_bundle_update_batch(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    batch_id: &str,
    now: i64,
    failpoint: LifecycleFailpoint,
) -> Result<(), BundleUpdateBatchError> {
    let batch = storage.read_bundle_update_batch(batch_id)?;
    if batch.status != "running" {
        return Ok(());
    }
    cleanup_not_executed_plans(paths, storage, &batch)?;
    if batch.items.iter().any(|item| item.status == "blocked") {
        let finished = storage.finish_bundle_update_batch(batch_id, "blocked", now)?;
        // 崩溃可能发生在 child 已记为 blocked、批次尚未收敛之间；本次恢复也要清理剩余 Plan。
        cleanup_not_executed_plans(paths, storage, &finished)?;
        return Ok(());
    }

    let mut selected = batch
        .items
        .iter()
        .filter(|item| item.confirmed_order.is_some())
        .cloned()
        .collect::<Vec<_>>();
    selected.sort_by_key(|item| item.confirmed_order);
    for item in selected {
        if item.status != "ready" {
            continue;
        }
        let transaction_status = item
            .install_plan_id
            .as_deref()
            .map(|plan_id| storage.install_plan_transaction_status(plan_id))
            .transpose()?
            .flatten();
        if transaction_status.as_deref() == Some("blocked") {
            block_batch_item(
                paths,
                storage,
                batch_id,
                &item,
                "Bundle 更新需要人工恢复",
                now,
            )?;
            return Ok(());
        }
        if transaction_status.as_deref() == Some("in_progress") {
            return Err(
                LifecycleError::SimulatedInterruption("Bundle 更新 child 事务仍在恢复").into(),
            );
        }
        if storage.bundle_has_adopted_marker(&item.bundle_id, &item.target_marker)? {
            storage.save_bundle_update_batch_item_result(
                batch_id,
                &item.id,
                "succeeded",
                None,
                now,
            )?;
            continue;
        }

        let Some(plan_id) = item.install_plan_id.as_deref() else {
            fail_batch_item(
                storage,
                batch_id,
                &item,
                "更新 Plan 已清理，但目标内容尚未采用",
                now,
            )?;
            continue;
        };
        let plan = match storage.read_install_plan(plan_id) {
            Ok(plan) if plan.status == "pending" => plan,
            Ok(_) | Err(StorageError::InstallPlanNotFound) => {
                fail_batch_item(
                    storage,
                    batch_id,
                    &item,
                    "更新 Plan 已不可执行，且目标内容尚未采用",
                    now,
                )?;
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        let selected_candidate_ids = required_update_candidate_ids(&plan)?;
        match confirm_bundle_update_batch_child_install(
            paths,
            storage,
            BundleUpdateBatchChildOwner {
                batch_id,
                item_id: &item.id,
            },
            plan_id,
            &selected_candidate_ids,
            now,
            failpoint,
        ) {
            Ok(()) => storage.save_bundle_update_batch_item_result(
                batch_id,
                &item.id,
                "succeeded",
                None,
                now,
            )?,
            Err(error) => {
                let transaction_status = storage.install_plan_transaction_status(plan_id)?;
                if transaction_status.as_deref() == Some("blocked")
                    || matches!(
                        error,
                        LifecycleError::RecoveryBlocked(_)
                            | LifecycleError::Storage(StorageError::ManagedObjectBlocked)
                    )
                {
                    block_batch_item(paths, storage, batch_id, &item, &error.to_string(), now)?;
                    return Ok(());
                }
                if transaction_status.as_deref() == Some("in_progress")
                    || matches!(error, LifecycleError::SimulatedInterruption(_))
                {
                    return Err(error.into());
                }
                discard_child_if_pending(paths, storage, batch_id, &item.id, plan_id)?;
                fail_batch_item(storage, batch_id, &item, &error.to_string(), now)?;
            }
        }
    }
    storage.finish_bundle_update_batch(batch_id, "completed", now)?;
    Ok(())
}

fn cleanup_prepared_plans(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    items: &[PreparedBatchItem],
) -> Result<(), BundleUpdateBatchError> {
    for plan_id in items
        .iter()
        .filter_map(|item| item.install_plan_id.as_deref())
    {
        discard_install_plan(paths, storage, plan_id)?;
    }
    Ok(())
}

fn cleanup_not_executed_plans(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    batch: &StoredBundleUpdateBatch,
) -> Result<(), BundleUpdateBatchError> {
    for item in batch
        .items
        .iter()
        .filter(|item| item.status == "not_executed")
    {
        if let Some(plan_id) = item.install_plan_id.as_deref() {
            discard_child_if_pending(paths, storage, &batch.id, &item.id, plan_id)?;
        }
    }
    Ok(())
}

fn discard_child_if_pending(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    batch_id: &str,
    item_id: &str,
    plan_id: &str,
) -> Result<(), BundleUpdateBatchError> {
    match storage.read_install_plan(plan_id) {
        Ok(plan) if plan.status == "pending" => {
            discard_bundle_update_batch_child_plan(
                paths,
                storage,
                BundleUpdateBatchChildOwner { batch_id, item_id },
                plan_id,
            )?;
        }
        Ok(_) | Err(StorageError::InstallPlanNotFound) => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn required_update_candidate_ids(
    plan: &StoredInstallPlan,
) -> Result<Vec<String>, BundleUpdateBatchError> {
    if plan.install_mode != "update" {
        return Err(StorageError::InvalidInstallPlan.into());
    }
    let ids = plan
        .candidates
        .iter()
        .filter(|candidate| !candidate.preserve_existing)
        .map(|candidate| candidate.candidate_id.clone())
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Err(StorageError::InvalidInstallPlan.into());
    }
    Ok(ids)
}

fn fail_batch_item(
    storage: &mut Storage,
    batch_id: &str,
    item: &StoredBundleUpdateBatchItem,
    error: &str,
    now: i64,
) -> Result<(), BundleUpdateBatchError> {
    storage.save_bundle_update_batch_item_result(batch_id, &item.id, "failed", Some(error), now)?;
    Ok(())
}

fn block_batch_item(
    paths: &ApplicationPaths,
    storage: &mut Storage,
    batch_id: &str,
    item: &StoredBundleUpdateBatchItem,
    error: &str,
    now: i64,
) -> Result<(), BundleUpdateBatchError> {
    storage.save_bundle_update_batch_item_result(
        batch_id,
        &item.id,
        "blocked",
        Some(error),
        now,
    )?;
    let finished = storage.finish_bundle_update_batch(batch_id, "blocked", now)?;
    cleanup_not_executed_plans(paths, storage, &finished)?;
    Ok(())
}

fn render_batch_plan(
    paths: &ApplicationPaths,
    storage: &Storage,
    batch: &StoredBundleUpdateBatch,
) -> Result<BundleUpdateBatchPlan, BundleUpdateBatchError> {
    if batch.status != "pending" {
        return Err(StorageError::InvalidBundleUpdateBatch.into());
    }
    let items = batch
        .items
        .iter()
        .map(|item| match item.status.as_str() {
            "ready" => {
                let plan_id = item
                    .install_plan_id
                    .as_deref()
                    .ok_or(StorageError::InvalidBundleUpdateBatch)?;
                let stored = storage.read_install_plan(plan_id)?;
                Ok(BundleUpdateBatchPlanItem {
                    id: item.id.clone(),
                    bundle_id: item.bundle_id.clone(),
                    bundle_display_name: item.display_name.clone(),
                    disposition: BundleUpdateBatchPlanItemDisposition::Ready,
                    install_plan: Some(render_install_plan(paths, storage, item, &stored)?),
                    error_summary: None,
                })
            }
            "preparation_failed" => Ok(BundleUpdateBatchPlanItem {
                id: item.id.clone(),
                bundle_id: item.bundle_id.clone(),
                bundle_display_name: item.display_name.clone(),
                disposition: BundleUpdateBatchPlanItemDisposition::PreparationFailed,
                install_plan: None,
                error_summary: item.error.clone(),
            }),
            _ => Err(StorageError::InvalidBundleUpdateBatch.into()),
        })
        .collect::<Result<Vec<_>, BundleUpdateBatchError>>()?;
    Ok(BundleUpdateBatchPlan {
        id: batch.id.clone(),
        items,
        created_at: batch.created_at,
        expires_at: batch.expires_at,
    })
}

fn render_install_plan(
    paths: &ApplicationPaths,
    storage: &Storage,
    item: &StoredBundleUpdateBatchItem,
    plan: &StoredInstallPlan,
) -> Result<InstallPlan, BundleUpdateBatchError> {
    if plan.status != "pending"
        || plan.install_mode != "update"
        || plan.bundle_id != item.bundle_id
        || plan.source_id.as_deref() != Some(item.source_id.as_str())
        || plan.source_marker.as_deref() != Some(item.target_marker.as_str())
    {
        return Err(StorageError::InvalidBundleUpdateBatch.into());
    }
    let (source_kind, locator) = storage.source_kind_and_locator(&item.source_id)?;
    let input_kind = match source_kind {
        SourceKind::Github => InstallInputKind::Github,
        SourceKind::EditableLocal => InstallInputKind::EditableLocal,
        SourceKind::Archive | SourceKind::DirectUrl => {
            return Err(StorageError::InvalidBundleUpdateBatch.into());
        }
    };
    let existing_member_ids = plan
        .candidates
        .iter()
        .filter(|candidate| candidate.previous_content_fingerprint.is_some())
        .map(|candidate| candidate.candidate_id.as_str())
        .collect::<BTreeSet<_>>();
    let existing_mounts = storage
        .read_mount_summaries()?
        .into_iter()
        .filter(|mount| existing_member_ids.contains(mount.member_id.as_str()))
        .collect::<Vec<_>>();
    let new_candidate_ids = plan
        .candidates
        .iter()
        .filter(|candidate| {
            !candidate.preserve_existing && candidate.previous_content_fingerprint.is_none()
        })
        .map(|candidate| candidate.candidate_id.clone())
        .collect::<Vec<_>>();
    let candidates = plan
        .candidates
        .iter()
        .filter(|candidate| !candidate.preserve_existing)
        .map(|candidate| {
            let source_relative_path = candidate
                .source_relative_path
                .clone()
                .ok_or(StorageError::InvalidInstallPlan)?;
            Ok(InstallCandidate {
                candidate_id: candidate.candidate_id.clone(),
                source_relative_path,
                skill_name: candidate.skill_name.clone(),
                description: candidate.skill_description.clone(),
                selectable: candidate.selectable,
                validation_errors: candidate.validation_errors.clone(),
                warnings: candidate.warnings.clone(),
                default_selected: candidate.default_selected,
                target_directory: candidate.skill_name.as_ref().map(|name| {
                    paths
                        .bundle_directory(&plan.bundle_id)
                        .join("current/members")
                        .join(name)
                        .display()
                        .to_string()
                }),
            })
        })
        .collect::<Result<Vec<_>, StorageError>>()?;
    Ok(InstallPlan {
        id: plan.id.clone(),
        input_kind,
        mode: InstallMode::Update,
        input_path: locator.clone(),
        bundle_display_name: plan.bundle_display_name.clone(),
        candidates,
        warnings: plan.warnings.clone(),
        will_mount: false,
        update_impact: Some(BundleUpdateImpact {
            new_candidate_ids,
            existing_mounts,
            upstream_url: (source_kind == SourceKind::Github).then_some(locator),
        }),
        created_at: plan.created_at,
        expires_at: plan.expires_at,
    })
}

fn render_batch_result(
    batch: &StoredBundleUpdateBatch,
) -> Result<BundleUpdateBatchResult, BundleUpdateBatchError> {
    let status = match batch.status.as_str() {
        "completed" => BundleUpdateBatchResultStatus::Completed,
        "blocked" => BundleUpdateBatchResultStatus::Blocked,
        _ => return Err(StorageError::InvalidBundleUpdateBatch.into()),
    };
    let mut stored_items = batch.items.iter().collect::<Vec<_>>();
    stored_items.sort_by_key(|item| {
        (
            item.confirmed_order.is_none(),
            item.confirmed_order.unwrap_or(item.display_order),
            item.display_order,
        )
    });
    let items = stored_items
        .into_iter()
        .map(|item| {
            let status = match item.status.as_str() {
                "succeeded" => BundleUpdateBatchResultItemStatus::Succeeded,
                "failed" => BundleUpdateBatchResultItemStatus::Failed,
                "blocked" => BundleUpdateBatchResultItemStatus::Blocked,
                "not_executed" => BundleUpdateBatchResultItemStatus::NotExecuted,
                _ => return Err(StorageError::InvalidBundleUpdateBatch.into()),
            };
            Ok(BundleUpdateBatchResultItem {
                id: item.id.clone(),
                bundle_id: item.bundle_id.clone(),
                bundle_display_name: item.display_name.clone(),
                status,
                error_summary: item.error.clone(),
            })
        })
        .collect::<Result<Vec<_>, BundleUpdateBatchError>>()?;
    Ok(BundleUpdateBatchResult {
        id: batch.id.clone(),
        status,
        items,
        confirmed_at: batch
            .confirmed_at
            .ok_or(StorageError::InvalidBundleUpdateBatch)?,
        updated_at: batch.updated_at,
    })
}
