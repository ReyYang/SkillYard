use std::sync::{
    Arc, Mutex, TryLockError,
    atomic::{AtomicBool, Ordering},
};

use thiserror::Error;

use crate::{
    domain::{PlatformInfo, TakeoverPlanRequest, UiIntent, UiOutcome},
    github_source::{
        GithubCatalogTarget, GithubSourceError, ReqwestSourceTransport, SharedSourceTransport,
        fetch_github_catalog, parse_github_source, resolve_github_source,
    },
    lifecycle::{
        LifecycleError, LifecycleFailpoint, LifecycleLock, acquire_lifecycle_lock,
        confirm_folder_install, create_folder_install_plan, ensure_central_store_layout,
        recover_pending_transactions,
    },
    mount_lifecycle::{
        MountLifecycleError, confirm_batch_mount_plan, confirm_mount_plan, create_batch_mount_plan,
        create_mount_plan, create_remove_mount_plan, create_repair_mount_plan,
        observe_mount_health, prepare_project_registration,
        recover_pending_batch_mount_transactions, recover_pending_mount_transactions,
        refresh_mount_health,
    },
    paths::ApplicationPaths,
    scanner::{scan, scan_projects, scan_with_projects},
    storage::{
        NewGitHubSource, NewSourceCatalogMember, SaveGitHubSourceResult, Storage, StorageError,
        StoredGithubSource,
    },
    takeover::{
        TakeoverError, confirm_takeover_plan, create_takeover_plan,
        recover_pending_takeover_transactions,
    },
};

const SOURCE_REF_PLAN_TTL_MILLIS: i64 = 30 * 60 * 1_000;

fn default_source_transport() -> Option<SharedSourceTransport> {
    // HTTP client 初始化失败也必须等到用户真正访问 Source 时作为普通错误呈现。
    ReqwestSourceTransport::new()
        .ok()
        .map(|transport| Arc::new(transport) as SharedSourceTransport)
}

#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
    #[error(transparent)]
    MountLifecycle(#[from] MountLifecycleError),
    #[error(transparent)]
    Takeover(#[from] TakeoverError),
    #[error(transparent)]
    GithubSource(#[from] GithubSourceError),
    #[error("首次扫描未完整完成：{0}")]
    InitialScan(String),
    #[error("当前状态不能执行这个操作：{0}")]
    InvalidState(&'static str),
    #[error("已有一项写操作正在执行，请等待完成")]
    OperationInProgress,
    #[error("写操作保护状态不可用，请重新启动 SkillYard")]
    OperationGateUnavailable,
}

/// 所有业务行为都从这个 seam 进入；Tauri command 只负责薄适配。
pub struct SkillYardApplication {
    paths: ApplicationPaths,
    platform: PlatformInfo,
    operation_gate: Mutex<()>,
    lifecycle_failpoint: LifecycleFailpoint,
    source_transport: Option<SharedSourceTransport>,
    source_discovery_loaded: AtomicBool,
}

impl SkillYardApplication {
    pub fn new(paths: ApplicationPaths, platform: PlatformInfo) -> Self {
        Self::new_with_dependencies(
            paths,
            platform,
            LifecycleFailpoint::None,
            default_source_transport(),
        )
    }

    /// 仅供崩溃恢复测试在精确阶段注入中断，生产入口始终使用 `None`。
    #[doc(hidden)]
    pub fn new_with_lifecycle_failpoint(
        paths: ApplicationPaths,
        platform: PlatformInfo,
        lifecycle_failpoint: LifecycleFailpoint,
    ) -> Self {
        Self::new_with_dependencies(
            paths,
            platform,
            lifecycle_failpoint,
            default_source_transport(),
        )
    }

    /// Source 协议测试只替换最外层 HTTP 读取，GitHub 解析和持久化仍走生产入口。
    #[doc(hidden)]
    pub fn new_with_source_transport(
        paths: ApplicationPaths,
        platform: PlatformInfo,
        source_transport: SharedSourceTransport,
    ) -> Self {
        Self::new_with_dependencies(
            paths,
            platform,
            LifecycleFailpoint::None,
            Some(source_transport),
        )
    }

    fn new_with_dependencies(
        paths: ApplicationPaths,
        platform: PlatformInfo,
        lifecycle_failpoint: LifecycleFailpoint,
        source_transport: Option<SharedSourceTransport>,
    ) -> Self {
        // SQLite 延迟到 intent 中打开，确保初始化失败能通过 UiError 呈现，而不是在窗口创建前 panic。
        Self {
            paths,
            platform,
            operation_gate: Mutex::new(()),
            lifecycle_failpoint,
            source_transport,
            source_discovery_loaded: AtomicBool::new(false),
        }
    }

    pub fn handle(&self, intent: UiIntent) -> Result<UiOutcome, ApplicationError> {
        if !self.platform.is_supported() {
            return Ok(UiOutcome::UnsupportedPlatform {
                actual_os: self.platform.os.clone(),
                actual_architecture: self.platform.architecture.clone(),
                actual_major_version: self.platform.major_version,
                required_architecture: "aarch64".to_owned(),
                minimum_major_version: 14,
            });
        }

        match intent {
            // 启动读取前可能需要修复上次中断的写事务，因此也必须取得单写门。
            UiIntent::GetStartupState => self.with_write_operation(|| self.get_startup_state()),
            UiIntent::StartInitialScan => self.with_write_operation(|| self.start_initial_scan()),
            UiIntent::RefreshLocalInventory => {
                self.with_write_operation(|| self.refresh_local_inventory())
            }
            UiIntent::OpenSourceDiscovery => {
                self.with_write_operation(|| self.open_source_discovery())
            }
            UiIntent::ReloadGitHubSource { source_id } => {
                self.with_write_operation(|| self.reload_github_source(source_id))
            }
            UiIntent::AddGitHubSource { input, tracked_ref } => {
                self.with_write_operation(|| self.add_github_source(input, tracked_ref))
            }
            UiIntent::ConfirmSourceRefChange { plan_id } => {
                self.with_write_operation(|| self.confirm_source_ref_change(plan_id))
            }
            UiIntent::CreateFolderInstallPlan { input_path } => {
                self.with_write_operation(|| self.create_folder_install_plan(input_path))
            }
            UiIntent::ConfirmInstallPlan {
                plan_id,
                selected_candidate_ids,
            } => self.with_write_operation(|| {
                self.confirm_install_plan(plan_id, selected_candidate_ids)
            }),
            UiIntent::RegisterProject { root_path } => {
                self.with_write_operation(|| self.register_project(root_path))
            }
            UiIntent::CreateTakeoverPlan { request } => {
                self.with_write_operation(|| self.create_takeover_plan(request))
            }
            UiIntent::ConfirmTakeoverPlan { plan_id } => {
                self.with_write_operation(|| self.confirm_takeover_plan(plan_id))
            }
            UiIntent::CreateMountPlan {
                member_id,
                app_id,
                scope,
                project_id,
            } => self.with_write_operation(|| {
                self.create_mount_plan(member_id, app_id, scope, project_id)
            }),
            UiIntent::CreateRemoveMountPlan { mount_id } => {
                self.with_write_operation(|| self.create_remove_mount_plan(mount_id))
            }
            UiIntent::CreateRepairMountPlan { mount_id } => {
                self.with_write_operation(|| self.create_repair_mount_plan(mount_id))
            }
            UiIntent::ConfirmMountPlan { plan_id } => {
                self.with_write_operation(|| self.confirm_mount_plan(plan_id))
            }
            UiIntent::CreateBatchMountPlan {
                bundle_id,
                requests,
            } => self.with_write_operation(|| self.create_batch_mount_plan(bundle_id, requests)),
            UiIntent::ConfirmBatchMountPlan {
                plan_id,
                selected_item_ids,
            } => self
                .with_write_operation(|| self.confirm_batch_mount_plan(plan_id, selected_item_ids)),
        }
    }

    /// 扫描结果会写入同一份 SQLite；拒绝并发写可避免旧快照覆盖新状态。
    fn with_write_operation(
        &self,
        operation: impl FnOnce() -> Result<UiOutcome, ApplicationError>,
    ) -> Result<UiOutcome, ApplicationError> {
        let _guard = match self.operation_gate.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => return Err(ApplicationError::OperationInProgress),
            Err(TryLockError::Poisoned(_)) => {
                return Err(ApplicationError::OperationGateUnavailable);
            }
        };
        operation()
    }

    fn get_startup_state(&self) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        if storage.read_initial_scan()?.is_some() {
            let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
            lifecycle_lock.recheck(&self.paths)?;
            refresh_mount_health(&self.paths, &mut storage, unix_timestamp_millis())?;
            lifecycle_lock.recheck(&self.paths)?;
            return storage
                .read_initial_scan()?
                .ok_or(ApplicationError::InvalidState("首次扫描状态已经丢失"));
        }

        Ok(UiOutcome::onboarding_required())
    }

    fn start_initial_scan(&self) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        // 恢复完成后继续持有跨进程锁，避免另一实例在扫描与保存之间改写同一份清单。
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        if let Some(outcome) = storage.read_initial_scan()? {
            lifecycle_lock.recheck(&self.paths)?;
            return Ok(outcome);
        }

        let result = scan(&self.paths);
        if let Some(issue) = result.issues.first() {
            return Err(ApplicationError::InitialScan(issue.message.clone()));
        }
        let scan_completed_at = unix_timestamp_millis();
        storage.save_initial_scan(scan_completed_at, &result.entries, &result.supported_apps)?;
        lifecycle_lock.recheck(&self.paths)?;
        storage
            .read_initial_scan()?
            .ok_or(ApplicationError::InvalidState("首次扫描结果没有保存成功"))
    }

    fn refresh_local_inventory(&self) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        // 刷新必须把读取旧状态、扫描和保存视为一次完整写操作。
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        let Some(UiOutcome::Inventory {
            scan_completed_at, ..
        }) = storage.read_initial_scan()?
        else {
            return Err(ApplicationError::InvalidState("完成首次扫描后才能刷新本机"));
        };

        let completed_at = unix_timestamp_millis();
        let mount_health = observe_mount_health(&self.paths, &storage)?;
        let mount_targets = storage.mount_target_paths()?;
        let projects = storage.read_stored_projects()?;
        let result = scan_with_projects(&self.paths, &projects, &mount_targets);
        let saved = storage.save_local_refresh(
            completed_at,
            &result.entries,
            &result.supported_apps,
            &result.successful_roots,
            &result.issues,
            &mount_health,
        )?;
        lifecycle_lock.recheck(&self.paths)?;

        Ok(UiOutcome::Inventory {
            scan_completed_at,
            entries: saved.entries,
            supported_apps: saved.supported_apps,
            last_local_refresh: Some(saved.summary),
            scan_issues: result.issues,
            recovery_issues: saved.recovery_issues,
            projects: saved.projects,
            mounts: saved.mounts,
        })
    }

    fn open_source_discovery(&self) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let should_reload = !self.source_discovery_loaded.load(Ordering::Acquire);
        if should_reload {
            let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
            lifecycle_lock.recheck(&self.paths)?;
            for source in storage.read_github_sources()? {
                self.reload_source_catalog(&mut storage, &source, &lifecycle_lock)?;
                lifecycle_lock.recheck(&self.paths)?;
            }
        }
        let sources = storage.read_source_summaries()?;
        if should_reload {
            self.source_discovery_loaded.store(true, Ordering::Release);
        }
        Ok(UiOutcome::SourceDiscovery {
            sources,
            highlighted_source_id: None,
            highlighted_member_path: None,
        })
    }

    fn reload_github_source(&self, source_id: String) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        let source = storage.read_github_source(&source_id)?;
        self.reload_source_catalog(&mut storage, &source, &lifecycle_lock)?;
        lifecycle_lock.recheck(&self.paths)?;
        let sources = storage.read_source_summaries()?;
        let highlighted_member_path = sources
            .iter()
            .find(|source| source.id == source_id)
            .and_then(|source| source.member_path_hint.clone());
        Ok(UiOutcome::SourceDiscovery {
            sources,
            highlighted_source_id: Some(source_id),
            highlighted_member_path,
        })
    }

    fn reload_source_catalog(
        &self,
        storage: &mut Storage,
        source: &StoredGithubSource,
        lifecycle_lock: &LifecycleLock,
    ) -> Result<(), ApplicationError> {
        lifecycle_lock.recheck(&self.paths)?;
        let fetched = self
            .source_transport
            .as_deref()
            .ok_or(GithubSourceError::Network)
            .and_then(|transport| {
                fetch_github_catalog(
                    transport,
                    GithubCatalogTarget {
                        owner: &source.owner,
                        repository: &source.repository,
                        canonical_identity: &source.canonical_identity,
                        display_name: &source.display_name,
                        tracked_ref: &source.tracked_ref,
                    },
                    &self.paths.staging_root(),
                )
            });
        lifecycle_lock.recheck(&self.paths)?;
        let completed_at = unix_timestamp_millis();
        match fetched {
            Ok(fetched) => {
                let ids = fetched
                    .candidates
                    .iter()
                    .map(|_| uuid::Uuid::new_v4().to_string())
                    .collect::<Vec<_>>();
                let relative_paths = fetched
                    .candidates
                    .iter()
                    .map(|candidate| {
                        candidate
                            .relative_path
                            .to_str()
                            .map(str::to_owned)
                            .ok_or(GithubSourceError::InvalidResponse)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let members = fetched
                    .candidates
                    .iter()
                    .enumerate()
                    .map(|(index, candidate)| NewSourceCatalogMember {
                        id: &ids[index],
                        relative_path: &relative_paths[index],
                        skill_name: candidate.name.as_deref(),
                        description: candidate.description.as_deref(),
                        content_fingerprint: candidate.fingerprint.as_deref(),
                        selectable: candidate.selectable(),
                        validation_errors: &candidate.validation_errors,
                        warnings: &candidate.warnings,
                    })
                    .collect::<Vec<_>>();
                storage.save_source_catalog_success(
                    &source.id,
                    &source.tracked_ref,
                    &fetched.commit_sha,
                    completed_at,
                    &members,
                )?;
            }
            Err(error) => storage.save_source_catalog_failure(
                &source.id,
                &source.tracked_ref,
                completed_at,
                &error.to_string(),
            )?,
        }
        Ok(())
    }

    fn add_github_source(
        &self,
        input: String,
        tracked_ref: Option<String>,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        let transport = self
            .source_transport
            .as_deref()
            .ok_or(GithubSourceError::Network)?;
        let parsed = parse_github_source(&input, tracked_ref.as_deref())?;
        let canonical_identity_hint = format!(
            "github:{}/{}",
            parsed.owner.to_ascii_lowercase(),
            parsed.repository.to_ascii_lowercase()
        );
        // 已有 Source 的无 ref 入口只验证当前 Tracked Ref，不能跟随远端 default branch 漂移。
        let stored_ref = if parsed.tracked_ref.is_none() {
            storage.read_source_tracked_ref(&canonical_identity_hint)?
        } else {
            None
        };
        let resolved = resolve_github_source(
            transport,
            &input,
            parsed.tracked_ref.as_deref().or(stored_ref.as_deref()),
        )?;
        lifecycle_lock.recheck(&self.paths)?;
        let now = unix_timestamp_millis();
        let source_id = uuid::Uuid::new_v4().to_string();
        let ref_change_plan_id = uuid::Uuid::new_v4().to_string();
        let result = storage.save_or_prepare_github_source(
            NewGitHubSource {
                id: &source_id,
                canonical_identity: &resolved.canonical_identity,
                owner: &resolved.owner,
                repository: &resolved.repository,
                display_name: &resolved.display_name,
                repository_url: resolved.repository_url.as_str(),
                tracked_ref: &resolved.tracked_ref,
                resolved_commit_sha: &resolved.commit,
                member_path_hint: resolved.member_hint.as_deref(),
            },
            &ref_change_plan_id,
            now,
            now.saturating_add(SOURCE_REF_PLAN_TTL_MILLIS),
        )?;
        lifecycle_lock.recheck(&self.paths)?;
        match result {
            SaveGitHubSourceResult::Saved { source_id } => Ok(UiOutcome::SourceDiscovery {
                sources: storage.read_source_summaries()?,
                highlighted_source_id: Some(source_id),
                highlighted_member_path: resolved.member_hint,
            }),
            SaveGitHubSourceResult::RefChangeRequired { plan } => {
                Ok(UiOutcome::SourceRefChangePlan { plan })
            }
        }
    }

    fn confirm_source_ref_change(&self, plan_id: String) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        let source_id = storage.confirm_source_ref_change(&plan_id, unix_timestamp_millis())?;
        lifecycle_lock.recheck(&self.paths)?;
        let sources = storage.read_source_summaries()?;
        let highlighted_member_path = sources
            .iter()
            .find(|source| source.id == source_id)
            .and_then(|source| source.member_path_hint.clone());
        Ok(UiOutcome::SourceDiscovery {
            sources,
            highlighted_source_id: Some(source_id),
            highlighted_member_path,
        })
    }

    fn create_folder_install_plan(
        &self,
        input_path: String,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        // Plan 会校验源目录并写入 SQLite，整个过程不能与另一实例的安装交错。
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        ensure_onboarding_completed(&storage)?;
        let plan = create_folder_install_plan(
            &self.paths,
            &mut storage,
            std::path::Path::new(&input_path),
            unix_timestamp_millis(),
        )?;
        lifecycle_lock.recheck(&self.paths)?;
        Ok(UiOutcome::FolderInstallPlan { plan })
    }

    fn confirm_install_plan(
        &self,
        plan_id: String,
        selected_candidate_ids: Vec<String>,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        confirm_folder_install(
            &self.paths,
            &mut storage,
            &plan_id,
            &selected_candidate_ids,
            unix_timestamp_millis(),
            self.lifecycle_failpoint,
        )?;
        storage
            .read_initial_scan()?
            .ok_or(ApplicationError::InvalidState("首次扫描状态已经丢失"))
    }

    fn register_project(&self, root_path: String) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        let project = prepare_project_registration(
            &self.paths,
            &storage,
            std::path::Path::new(&root_path),
            unix_timestamp_millis(),
        )?;
        let mount_targets = storage.mount_target_paths()?;
        let result = scan_projects(&self.paths, std::slice::from_ref(&project), &mount_targets);
        storage.register_project_with_scan(
            &project,
            &result.entries,
            &result.successful_roots,
            &result.issues,
        )?;
        lifecycle_lock.recheck(&self.paths)?;
        storage
            .read_initial_scan()?
            .ok_or(ApplicationError::InvalidState("首次扫描状态已经丢失"))
    }

    fn create_takeover_plan(
        &self,
        request: TakeoverPlanRequest,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        let plan =
            create_takeover_plan(&self.paths, &mut storage, request, unix_timestamp_millis())?;
        lifecycle_lock.recheck(&self.paths)?;
        Ok(UiOutcome::TakeoverPlan { plan })
    }

    fn confirm_takeover_plan(&self, plan_id: String) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        confirm_takeover_plan(
            &self.paths,
            &mut storage,
            &plan_id,
            unix_timestamp_millis(),
            self.lifecycle_failpoint,
        )?;
        storage
            .read_initial_scan()?
            .ok_or(ApplicationError::InvalidState("首次扫描状态已经丢失"))
    }

    fn create_mount_plan(
        &self,
        member_id: String,
        app_id: crate::domain::SupportedAppId,
        scope: crate::domain::MountScope,
        project_id: Option<String>,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        let plan = create_mount_plan(
            &self.paths,
            &mut storage,
            &member_id,
            app_id,
            scope,
            project_id.as_deref(),
            unix_timestamp_millis(),
        )?;
        lifecycle_lock.recheck(&self.paths)?;
        Ok(UiOutcome::MountPlan { plan })
    }

    fn create_remove_mount_plan(&self, mount_id: String) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        let plan = create_remove_mount_plan(
            &self.paths,
            &mut storage,
            &mount_id,
            unix_timestamp_millis(),
        )?;
        lifecycle_lock.recheck(&self.paths)?;
        Ok(UiOutcome::MountPlan { plan })
    }

    fn create_repair_mount_plan(&self, mount_id: String) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        let plan = create_repair_mount_plan(
            &self.paths,
            &mut storage,
            &mount_id,
            unix_timestamp_millis(),
        )?;
        lifecycle_lock.recheck(&self.paths)?;
        Ok(UiOutcome::MountPlan { plan })
    }

    fn confirm_mount_plan(&self, plan_id: String) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        confirm_mount_plan(
            &self.paths,
            &mut storage,
            &plan_id,
            unix_timestamp_millis(),
            self.lifecycle_failpoint,
        )?;
        storage
            .read_initial_scan()?
            .ok_or(ApplicationError::InvalidState("首次扫描状态已经丢失"))
    }

    fn create_batch_mount_plan(
        &self,
        bundle_id: String,
        requests: Vec<crate::domain::BatchMountRequest>,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        let plan = create_batch_mount_plan(
            &self.paths,
            &mut storage,
            &bundle_id,
            &requests,
            unix_timestamp_millis(),
        )?;
        lifecycle_lock.recheck(&self.paths)?;
        Ok(UiOutcome::BatchMountPlan { plan })
    }

    fn confirm_batch_mount_plan(
        &self,
        plan_id: String,
        selected_item_ids: Vec<String>,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        confirm_batch_mount_plan(
            &self.paths,
            &mut storage,
            &plan_id,
            &selected_item_ids,
            unix_timestamp_millis(),
            self.lifecycle_failpoint,
        )?;
        storage
            .read_initial_scan()?
            .ok_or(ApplicationError::InvalidState("首次扫描状态已经丢失"))
    }

    fn open_recovered_storage(&self) -> Result<Storage, ApplicationError> {
        let mut storage = Storage::open(self.paths.data_root(), &self.paths.database())?;
        ensure_central_store_layout(&self.paths)?;
        recover_pending_transactions(&self.paths, &mut storage, unix_timestamp_millis())?;
        recover_pending_mount_transactions(&self.paths, &mut storage, unix_timestamp_millis())?;
        recover_pending_batch_mount_transactions(
            &self.paths,
            &mut storage,
            unix_timestamp_millis(),
        )?;
        recover_pending_takeover_transactions(
            &self.paths,
            &mut storage,
            unix_timestamp_millis(),
            self.lifecycle_failpoint,
        )?;
        Ok(storage)
    }
}

fn ensure_onboarding_completed(storage: &Storage) -> Result<(), ApplicationError> {
    if storage.read_initial_scan()?.is_some() {
        Ok(())
    } else {
        Err(ApplicationError::InvalidState(
            "完成首次扫描后才能安装 Skill",
        ))
    }
}

fn unix_timestamp_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("系统时间必须晚于 Unix epoch")
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn a_second_write_intent_is_rejected_while_the_operation_gate_is_held() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let application = SkillYardApplication::new(
            ApplicationPaths::for_home(sandbox.path().join("data"), sandbox.path().join("home")),
            PlatformInfo::supported_for_test(),
        );

        let _active_operation = application
            .operation_gate
            .lock()
            .expect("测试应取得写操作门");
        let error = application
            .handle(UiIntent::StartInitialScan)
            .expect_err("并发写操作必须被拒绝");

        assert!(matches!(error, ApplicationError::OperationInProgress));
    }

    #[test]
    fn an_external_lifecycle_lock_rejects_a_scan_before_it_writes_inventory() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let paths =
            ApplicationPaths::for_home(sandbox.path().join("data"), sandbox.path().join("home"));
        ensure_central_store_layout(&paths).expect("应准备 Central Store");
        let _external_lock = acquire_lifecycle_lock(&paths).expect("测试应取得跨进程锁");
        let application =
            SkillYardApplication::new(paths.clone(), PlatformInfo::supported_for_test());

        let error = application
            .handle(UiIntent::StartInitialScan)
            .expect_err("另一实例持锁时扫描必须被拒绝");

        assert!(matches!(
            error,
            ApplicationError::Lifecycle(LifecycleError::LifecycleBusy)
        ));
        let storage =
            Storage::open(paths.data_root(), &paths.database()).expect("应能只读核对测试数据库");
        assert!(
            storage
                .read_initial_scan()
                .expect("应能读取首次扫描状态")
                .is_none(),
            "跨进程锁冲突不能留下部分扫描结果"
        );
    }

    #[test]
    fn lifecycle_lock_detects_the_visible_central_store_being_replaced() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let paths =
            ApplicationPaths::for_home(sandbox.path().join("data"), sandbox.path().join("home"));
        ensure_central_store_layout(&paths).expect("应准备 Central Store");
        let lock = acquire_lifecycle_lock(&paths).expect("应取得生命周期锁");
        let moved = sandbox.path().join("moved-data");
        fs::rename(paths.data_root(), &moved).expect("应模拟移动 Central Store");
        fs::create_dir(paths.data_root()).expect("应模拟创建同名替代目录");

        let error = lock
            .recheck(&paths)
            .expect_err("锁必须绑定取得时的 Central Store inode");

        assert!(matches!(error, LifecycleError::UnsafeCentralStore(_)));
    }

    #[test]
    fn lifecycle_lock_detects_its_lock_file_being_replaced() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let paths =
            ApplicationPaths::for_home(sandbox.path().join("data"), sandbox.path().join("home"));
        ensure_central_store_layout(&paths).expect("应准备 Central Store");
        let lock = acquire_lifecycle_lock(&paths).expect("应取得生命周期锁");
        let lock_path = paths.data_root().join(".lifecycle.lock");
        fs::remove_file(&lock_path).expect("应模拟删除已持锁目录项");
        fs::write(&lock_path, []).expect("应模拟创建新锁文件");

        let error = lock
            .recheck(&paths)
            .expect_err("锁必须绑定取得时的文件 inode");

        assert!(matches!(error, LifecycleError::UnsafeCentralStore(_)));
    }
}
