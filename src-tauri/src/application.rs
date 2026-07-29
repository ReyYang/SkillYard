use std::{
    path::Path,
    sync::{Arc, Mutex, TryLockError},
};

use thiserror::Error;

use crate::{
    agent::{
        AgentError, AgentProviderEndpoints, KeychainSecretStore, SharedSecretStore, answer_agent,
        build_local_skill_catalog, generate_skill_ai_explanation, read_skill_material,
        search_public_skills, verify_provider,
    },
    bundle_update_batch::{
        BundleUpdateBatchError, acknowledge_bundle_update_batch, confirm_bundle_update_batch_plan,
        create_bundle_update_batch_plan, discard_bundle_update_batch_plan,
        read_open_bundle_update_batch_outcome, recover_running_bundle_update_batch,
    },
    domain::{
        AgentConversationMessage, AgentPageContext, AgentPageKind, AiPreferences, AiProvider,
        BundleUpdateStatus, EditableLocalRelinkMember, InterfaceLanguage, MergeContentChoice,
        PlatformInfo, SourceKind, SourceMemberMappingChoice, TakeoverPlanRequest, UiIntent,
        UiOutcome,
    },
    github_source::{
        GithubCatalogTarget, GithubSourceError, ReqwestSourceTransport, SharedSourceTransport,
        fetch_github_catalog, parse_github_source, resolve_github_source,
    },
    lifecycle::{
        LifecycleError, LifecycleFailpoint, LifecycleLock, acquire_lifecycle_lock,
        check_editable_local_bundle as check_editable_local_bundle_lifecycle, confirm_install,
        create_archive_install_plan, create_bundle_replacement_plan, create_bundle_update_plan,
        create_editable_local_install_plan, create_folder_install_plan, create_github_install_plan,
        create_url_install_plan, discard_install_plan, ensure_central_store_layout,
        recover_pending_transactions, write_notice_from_storage,
    },
    mount_lifecycle::{
        MountLifecycleError, confirm_batch_mount_plan, confirm_mount_plan, create_batch_mount_plan,
        create_mount_plan, create_remove_mount_plan, create_repair_mount_plan,
        observe_mount_health, prepare_project_registration,
        recover_pending_batch_mount_transactions, recover_pending_mount_transactions,
        refresh_mount_health,
    },
    paths::ApplicationPaths,
    removal::{
        RemovalError, confirm_removal_plan, create_bundle_mount_removal_plan,
        create_bundle_removal_plan, create_project_removal_plan, create_source_removal_plan,
        discard_removal_plan, read_open_removal_plan, recover_pending_removals,
    },
    scanner::{scan, scan_projects, scan_with_projects},
    skills_sh::{SkillsShError, search_skills_sh},
    source_association::{
        SourceAssociationError, confirm_source_association_plan, create_source_association_plan,
        discard_source_association_plan, recover_pending_source_association_transactions,
    },
    source_input::{SourceFilesystemIdentity, SourceInputError, inspect_editable_local_source},
    storage::{
        NewEditableLocalRelinkPlan, NewGitHubSource, NewSourceCatalogMember,
        SaveGitHubSourceResult, Storage, StorageError, StoredGithubSource,
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
    #[error(transparent)]
    SkillsSh(#[from] SkillsShError),
    #[error(transparent)]
    SourceAssociation(#[from] SourceAssociationError),
    #[error(transparent)]
    BundleUpdateBatch(#[from] BundleUpdateBatchError),
    #[error(transparent)]
    Removal(#[from] RemovalError),
    #[error(transparent)]
    Agent(#[from] AgentError),
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
    secret_store: SharedSecretStore,
    agent_endpoints: AgentProviderEndpoints,
}

impl SkillYardApplication {
    pub fn new(paths: ApplicationPaths, platform: PlatformInfo) -> Self {
        Self::new_with_dependencies(
            paths,
            platform,
            LifecycleFailpoint::None,
            default_source_transport(),
            Arc::new(KeychainSecretStore),
            AgentProviderEndpoints::production(),
        )
    }

    /// Finder 入口只暴露固定的持久用户内容目录，不接受前端路径。
    pub(crate) fn central_store_path(&self) -> &Path {
        self.paths.data_root()
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
            Arc::new(KeychainSecretStore),
            AgentProviderEndpoints::production(),
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
            Arc::new(KeychainSecretStore),
            AgentProviderEndpoints::production(),
        )
    }

    /// Provider 合同测试替换 Keychain 与固定 endpoint，但所有行为仍通过唯一 Application seam。
    #[doc(hidden)]
    pub fn new_with_agent_dependencies(
        paths: ApplicationPaths,
        platform: PlatformInfo,
        secret_store: SharedSecretStore,
        agent_endpoints: AgentProviderEndpoints,
    ) -> Self {
        Self::new_with_dependencies(
            paths,
            platform,
            LifecycleFailpoint::None,
            default_source_transport(),
            secret_store,
            agent_endpoints,
        )
    }

    fn new_with_dependencies(
        paths: ApplicationPaths,
        platform: PlatformInfo,
        lifecycle_failpoint: LifecycleFailpoint,
        source_transport: Option<SharedSourceTransport>,
        secret_store: SharedSecretStore,
        agent_endpoints: AgentProviderEndpoints,
    ) -> Self {
        // SQLite 延迟到 intent 中打开，确保初始化失败能通过 UiError 呈现，而不是在窗口创建前 panic。
        Self {
            paths,
            platform,
            operation_gate: Mutex::new(()),
            lifecycle_failpoint,
            source_transport,
            secret_store,
            agent_endpoints,
        }
    }

    pub fn handle(&self, intent: UiIntent) -> Result<UiOutcome, ApplicationError> {
        // 偏好需要在平台阻塞页之前可读，确保该页面也能使用用户已经选择的语言。
        match &intent {
            UiIntent::GetPreferences => return self.get_preferences(),
            UiIntent::SetInterfaceLanguage { language } => {
                return self.with_write_operation(|| self.set_interface_language(*language));
            }
            UiIntent::SetAiConfiguration {
                enabled,
                disclosure_accepted,
                provider,
                model,
            } => {
                return self.with_write_operation(|| {
                    self.set_ai_configuration(
                        *enabled,
                        *disclosure_accepted,
                        *provider,
                        model.clone(),
                    )
                });
            }
            UiIntent::SaveAiApiKey { api_key } => {
                return self.with_write_operation(|| self.save_ai_api_key(api_key.as_str()));
            }
            UiIntent::DeleteAiApiKey => {
                return self.with_write_operation(|| self.delete_ai_api_key());
            }
            UiIntent::TestAiConnection => {
                return self.with_write_operation(|| self.test_ai_connection());
            }
            _ => {}
        }

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
            UiIntent::GetPreferences
            | UiIntent::SetInterfaceLanguage { .. }
            | UiIntent::SetAiConfiguration { .. }
            | UiIntent::SaveAiApiKey { .. }
            | UiIntent::DeleteAiApiKey
            | UiIntent::TestAiConnection => {
                unreachable!("偏好意图已在平台检查前处理")
            }
            UiIntent::AskAgent { context, messages } => self.ask_agent(context, messages),
            UiIntent::GenerateSkillAiExplanation { inventory_id } => {
                self.generate_skill_ai_explanation(inventory_id)
            }
            UiIntent::OrganizeSkillAiExplanations => self.organize_skill_ai_explanations(),
            // 启动读取前可能需要修复上次中断的写事务，因此也必须取得单写门。
            UiIntent::GetStartupState => self.with_write_operation(|| self.get_startup_state()),
            UiIntent::StartInitialScan => self.with_write_operation(|| self.start_initial_scan()),
            UiIntent::RefreshLocalInventory => {
                self.with_write_operation(|| self.refresh_local_inventory())
            }
            UiIntent::CheckBundleUpdates => {
                self.with_write_operation(|| self.check_bundle_updates())
            }
            UiIntent::CheckEditableLocalBundle { bundle_id } => {
                self.with_write_operation(|| self.check_editable_local_bundle(bundle_id))
            }
            UiIntent::OpenSourceDiscovery => {
                self.with_write_operation(|| self.open_source_discovery())
            }
            UiIntent::SearchSkillsSh { query } => {
                self.with_write_operation(|| self.search_skills_sh(query))
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
            UiIntent::CreateEditableLocalRelinkPlan {
                source_id,
                candidate_path,
            } => self.with_write_operation(|| {
                self.create_editable_local_relink_plan(source_id, candidate_path)
            }),
            UiIntent::ConfirmEditableLocalRelinkPlan { plan_id } => {
                self.with_write_operation(|| self.confirm_editable_local_relink_plan(plan_id))
            }
            UiIntent::DiscardEditableLocalRelinkPlan { plan_id } => {
                self.with_write_operation(|| self.discard_editable_local_relink_plan(plan_id))
            }
            UiIntent::CreateFolderInstallPlan { input_path } => {
                self.with_write_operation(|| self.create_folder_install_plan(input_path))
            }
            UiIntent::CreateArchiveInstallPlan { input_path } => {
                self.with_write_operation(|| self.create_archive_install_plan(input_path))
            }
            UiIntent::CreateUrlInstallPlan { url } => {
                self.with_write_operation(|| self.create_url_install_plan(url))
            }
            UiIntent::CreateEditableLocalInstallPlan { input_path } => {
                self.with_write_operation(|| self.create_editable_local_install_plan(input_path))
            }
            UiIntent::CreateGithubInstallPlan { source_id } => {
                self.with_write_operation(|| self.create_github_install_plan(source_id))
            }
            UiIntent::CreateBundleUpdatePlan { bundle_id } => {
                self.with_write_operation(|| self.create_bundle_update_plan(bundle_id))
            }
            UiIntent::CreateBundleUpdateBatchPlan => {
                self.with_write_operation(|| self.create_bundle_update_batch_plan())
            }
            UiIntent::ConfirmBundleUpdateBatchPlan {
                plan_id,
                selected_item_ids,
            } => self.with_write_operation(|| {
                self.confirm_bundle_update_batch_plan(plan_id, selected_item_ids)
            }),
            UiIntent::DiscardBundleUpdateBatchPlan { plan_id } => {
                self.with_write_operation(|| self.discard_bundle_update_batch_plan(plan_id))
            }
            UiIntent::AcknowledgeBundleUpdateBatch { batch_id } => {
                self.with_write_operation(|| self.acknowledge_bundle_update_batch(batch_id))
            }
            UiIntent::CreateBundleReplacementPlan {
                bundle_id,
                input_path,
            } => self.with_write_operation(|| {
                self.create_bundle_replacement_plan(bundle_id, input_path)
            }),
            UiIntent::ConfirmInstallPlan {
                plan_id,
                selected_candidate_ids,
            } => self.with_write_operation(|| {
                self.confirm_install_plan(plan_id, selected_candidate_ids)
            }),
            UiIntent::DiscardInstallPlan { plan_id } => {
                self.with_write_operation(|| self.discard_install_plan(plan_id))
            }
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
            UiIntent::CreateProjectRemovalPlan { project_id } => {
                self.with_write_operation(|| self.create_project_removal_plan(project_id))
            }
            UiIntent::CreateSourceRemovalPlan { source_id } => {
                self.with_write_operation(|| self.create_source_removal_plan(source_id))
            }
            UiIntent::CreateBundleRemovalPlan { bundle_id } => {
                self.with_write_operation(|| self.create_bundle_removal_plan(bundle_id))
            }
            UiIntent::CreateBundleMountRemovalPlan { bundle_id } => {
                self.with_write_operation(|| self.create_bundle_mount_removal_plan(bundle_id))
            }
            UiIntent::ConfirmRemovalPlan { plan_id } => {
                self.with_write_operation(|| self.confirm_removal_plan(plan_id))
            }
            UiIntent::DiscardRemovalPlan { plan_id } => {
                self.with_write_operation(|| self.discard_removal_plan(plan_id))
            }
            UiIntent::CreateSourceAssociationPlan {
                bundle_id,
                source_id,
                member_choices,
            } => self.with_write_operation(|| {
                self.create_source_association_plan(bundle_id, source_id, member_choices)
            }),
            UiIntent::ConfirmSourceAssociationPlan {
                plan_id,
                content_choices,
            } => self.with_write_operation(|| {
                self.confirm_source_association_plan(plan_id, content_choices)
            }),
            UiIntent::DiscardSourceAssociationPlan { plan_id } => {
                self.with_write_operation(|| self.discard_source_association_plan(plan_id))
            }
        }
    }

    fn get_preferences(&self) -> Result<UiOutcome, ApplicationError> {
        let storage = self.open_storage()?;
        Ok(UiOutcome::Preferences {
            language: storage.read_interface_language()?,
            ai: self.read_ai_preferences(&storage)?,
        })
    }

    fn set_interface_language(
        &self,
        language: InterfaceLanguage,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_storage()?;
        storage.save_interface_language(language)?;
        Ok(UiOutcome::Preferences {
            language,
            ai: self.read_ai_preferences(&storage)?,
        })
    }

    fn set_ai_configuration(
        &self,
        enabled: bool,
        disclosure_accepted: bool,
        provider: AiProvider,
        model: String,
    ) -> Result<UiOutcome, ApplicationError> {
        if !provider.supports_model(&model) {
            return Err(AgentError::UnsupportedModel(model).into());
        }
        let mut storage = self.open_storage()?;
        storage.save_ai_configuration(enabled, disclosure_accepted, provider, &model)?;
        self.preferences_outcome(&storage)
    }

    fn save_ai_api_key(&self, api_key: &str) -> Result<UiOutcome, ApplicationError> {
        if api_key.trim().is_empty() {
            return Err(AgentError::MissingApiKey.into());
        }
        let mut storage = self.open_storage()?;
        let ai = storage.read_ai_preferences()?;
        self.secret_store
            .write(ai.provider.api_key_account(), api_key.trim())
            .map_err(AgentError::from)?;
        storage.set_ai_verified(false)?;
        self.preferences_outcome(&storage)
    }

    fn delete_ai_api_key(&self) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_storage()?;
        let ai = storage.read_ai_preferences()?;
        self.secret_store
            .delete(ai.provider.api_key_account())
            .map_err(AgentError::from)?;
        storage.set_ai_verified(false)?;
        self.preferences_outcome(&storage)
    }

    fn test_ai_connection(&self) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_storage()?;
        let ai = storage.read_ai_preferences()?;
        if !ai.disclosure_accepted {
            return Err(AgentError::DisclosureRequired.into());
        }
        let api_key = self
            .secret_store
            .read(ai.provider.api_key_account())
            .map_err(AgentError::from)?
            .ok_or(AgentError::MissingApiKey)?;
        // 先撤销旧验证，确保任一能力测试失败后不会保留过期的“已验证”状态。
        storage.set_ai_verified(false)?;
        verify_provider(
            &self.agent_endpoints,
            ai.provider,
            ai.model.as_str(),
            api_key.as_str(),
        )?;
        storage.set_ai_verified(true)?;
        self.preferences_outcome(&storage)
    }

    fn ask_agent(
        &self,
        context: AgentPageContext,
        messages: Vec<AgentConversationMessage>,
    ) -> Result<UiOutcome, ApplicationError> {
        let storage = self.open_storage()?;
        let ai = storage.read_ai_preferences()?;
        if !ai.enabled {
            return Err(AgentError::Disabled.into());
        }
        if !ai.disclosure_accepted {
            return Err(AgentError::DisclosureRequired.into());
        }
        if !ai.verified {
            return Err(AgentError::NotVerified.into());
        }
        let api_key = self
            .secret_store
            .read(ai.provider.api_key_account())
            .map_err(AgentError::from)?
            .ok_or(AgentError::MissingApiKey)?;
        let language = storage.read_interface_language()?;
        let inventory_entries = match storage.read_initial_scan()? {
            Some(UiOutcome::Inventory { entries, .. }) => entries,
            _ => Vec::new(),
        };
        let material = match context {
            AgentPageContext::Skill { inventory_id } => {
                let entry = inventory_entries
                    .iter()
                    .find(|entry| entry.id == inventory_id)
                    .ok_or(AgentError::SkillNotFound)?;
                read_skill_material(
                    &entry.skill_name,
                    Path::new(&entry.skill_root),
                    Path::new(&entry.skill_file),
                    self.paths.home(),
                )?
            }
            AgentPageContext::Page { page } => {
                let page = match page {
                    AgentPageKind::Onboarding => "Onboarding",
                    AgentPageKind::Inventory => "Bundle inventory",
                    AgentPageKind::Settings => "Settings",
                    AgentPageKind::SourceDiscovery => "Source discovery",
                    AgentPageKind::Operation => "Lifecycle preview or operation",
                    AgentPageKind::UnsupportedPlatform => "Unsupported platform",
                };
                format!("Current page: {page}. No Skill file is attached.")
            }
        };
        let catalog = build_local_skill_catalog(
            inventory_entries
                .iter()
                .filter(|entry| !entry.stale)
                .map(|entry| {
                    (
                        entry.skill_name.as_str(),
                        Path::new(&entry.skill_root),
                        Path::new(&entry.skill_file),
                    )
                }),
            self.paths.home(),
        );
        let context = format!("{material}\n\n{catalog}");
        let answer = answer_agent(
            &self.agent_endpoints,
            ai.provider,
            &ai.model,
            &api_key,
            language,
            &messages,
            &context,
        )?;
        let (reply, searched_public_web, search_results) = if answer.search_public {
            let query = messages
                .last()
                .map(|message| message.content.as_str())
                .ok_or(AgentError::EmptyConversation)?;
            let search = search_public_skills(
                &self.agent_endpoints,
                ai.provider,
                &ai.model,
                &api_key,
                language,
                query,
            )?;
            (search.reply, true, search.results)
        } else {
            (answer.reply, false, Vec::new())
        };
        Ok(UiOutcome::AgentReply {
            reply,
            local_match_found: answer.local_match_found,
            searched_public_web,
            search_results,
        })
    }

    fn generate_skill_ai_explanation(
        &self,
        inventory_id: String,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_storage()?;
        let (ai, api_key, language) = self.ready_ai_context(&storage)?;
        let outcome = storage
            .read_initial_scan()?
            .ok_or(AgentError::SkillNotFound)?;
        let UiOutcome::Inventory { entries, .. } = outcome else {
            return Err(AgentError::SkillNotFound.into());
        };
        let entry = entries
            .into_iter()
            .find(|entry| entry.id == inventory_id)
            .ok_or(AgentError::SkillNotFound)?;
        let explanation = self.generate_explanation_for_entry(&entry, &ai, &api_key, language)?;
        storage.save_skill_ai_explanation(&entry.id, &explanation)?;
        storage
            .read_initial_scan()?
            .ok_or(ApplicationError::InvalidState("首次扫描状态已经丢失"))
    }

    fn organize_skill_ai_explanations(&self) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_storage()?;
        let (ai, api_key, language) = self.ready_ai_context(&storage)?;
        let outcome = storage
            .read_initial_scan()?
            .ok_or(AgentError::SkillNotFound)?;
        let UiOutcome::Inventory { entries, .. } = outcome else {
            return Err(AgentError::SkillNotFound.into());
        };
        for entry in entries.into_iter().filter(|entry| {
            entry
                .ai_explanation
                .as_ref()
                .is_none_or(|explanation| explanation.stale)
        }) {
            // 每个 Skill 独立请求；文件或 Provider 单项失败只保留其待整理状态。
            let Ok(explanation) =
                self.generate_explanation_for_entry(&entry, &ai, &api_key, language)
            else {
                continue;
            };
            storage.save_skill_ai_explanation(&entry.id, &explanation)?;
        }
        storage
            .read_initial_scan()?
            .ok_or(ApplicationError::InvalidState("首次扫描状态已经丢失"))
    }

    fn ready_ai_context(
        &self,
        storage: &Storage,
    ) -> Result<(AiPreferences, String, InterfaceLanguage), ApplicationError> {
        let ai = storage.read_ai_preferences()?;
        if !ai.enabled {
            return Err(AgentError::Disabled.into());
        }
        if !ai.disclosure_accepted {
            return Err(AgentError::DisclosureRequired.into());
        }
        if !ai.verified {
            return Err(AgentError::NotVerified.into());
        }
        let api_key = self
            .secret_store
            .read(ai.provider.api_key_account())
            .map_err(AgentError::from)?
            .ok_or(AgentError::MissingApiKey)?;
        let language = storage.read_interface_language()?;
        Ok((ai, api_key, language))
    }

    fn generate_explanation_for_entry(
        &self,
        entry: &crate::domain::InventoryItem,
        ai: &AiPreferences,
        api_key: &str,
        language: InterfaceLanguage,
    ) -> Result<crate::domain::SkillAiExplanation, AgentError> {
        let material = read_skill_material(
            &entry.skill_name,
            Path::new(&entry.skill_root),
            Path::new(&entry.skill_file),
            self.paths.home(),
        )?;
        generate_skill_ai_explanation(
            &self.agent_endpoints,
            ai.provider,
            &ai.model,
            api_key,
            language,
            &entry.observed_fingerprint,
            &material,
        )
    }

    fn read_ai_preferences(&self, storage: &Storage) -> Result<AiPreferences, ApplicationError> {
        let mut ai = storage.read_ai_preferences()?;
        ai.has_api_key = self
            .secret_store
            .read(ai.provider.api_key_account())
            .map_err(AgentError::from)?
            .is_some();
        Ok(ai)
    }

    fn preferences_outcome(&self, storage: &Storage) -> Result<UiOutcome, ApplicationError> {
        Ok(UiOutcome::Preferences {
            language: storage.read_interface_language()?,
            ai: self.read_ai_preferences(storage)?,
        })
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
        let (mut storage, recovered_interrupted_operation) =
            self.open_recovered_storage_with_notice()?;
        if storage.read_initial_scan()?.is_some() {
            if let Some(plan) = read_open_removal_plan(&storage)? {
                return Ok(UiOutcome::RemovalPlan { plan });
            }
            if let Some(outcome) = read_open_bundle_update_batch_outcome(&self.paths, &storage)? {
                return Ok(outcome);
            }
            if let Some(plan) =
                storage.read_open_editable_local_relink_plan(unix_timestamp_millis())?
            {
                return Ok(UiOutcome::EditableLocalRelinkPlan { plan });
            }
            let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
            lifecycle_lock.recheck(&self.paths)?;
            refresh_mount_health(&self.paths, &mut storage, unix_timestamp_millis())?;
            lifecycle_lock.recheck(&self.paths)?;
            return storage
                .read_initial_scan()?
                .ok_or(ApplicationError::InvalidState("首次扫描状态已经丢失"))
                .map(|outcome| {
                    outcome.with_recovered_interrupted_operation(recovered_interrupted_operation)
                });
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
        let scan_completed_at = unix_timestamp_millis();
        storage.save_initial_scan(
            scan_completed_at,
            &result.entries,
            &result.supported_apps,
            &result.issues,
        )?;
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
            recovered_interrupted_operation: false,
            projects: saved.projects,
            mounts: saved.mounts,
            bundle_updates: storage.read_bundle_update_summaries()?,
        })
    }

    fn check_bundle_updates(&self) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        let sources = storage.read_github_bundle_update_sources()?;
        for source in sources {
            lifecycle_lock.recheck(&self.paths)?;
            if storage
                .source_install_object_is_blocked(&source.source_id, Some(&source.bundle_id))?
            {
                // 人工恢复中的对象保持原检查状态，也不触发无意义的上游请求。
                continue;
            }
            let resolved = self
                .source_transport
                .as_deref()
                .ok_or(GithubSourceError::Network)
                .and_then(|transport| {
                    resolve_github_source(transport, &source.locator, Some(&source.tracked_ref))
                })
                .and_then(|resolved| {
                    // 仓库重定向或 ref 漂移不能在一次检查中改写已登记 Source 身份。
                    if resolved.canonical_identity != source.canonical_identity
                        || resolved.tracked_ref != source.tracked_ref
                    {
                        Err(GithubSourceError::InvalidResponse)
                    } else {
                        Ok(resolved)
                    }
                });
            lifecycle_lock.recheck(&self.paths)?;
            let checked_at = unix_timestamp_millis();
            match resolved {
                Ok(resolved) => {
                    let status = if source.adopted_marker.as_deref() == Some(&resolved.commit) {
                        BundleUpdateStatus::UpToDate
                    } else {
                        BundleUpdateStatus::Available
                    };
                    storage.save_bundle_update_check_success(
                        &source.source_id,
                        &source.bundle_id,
                        status,
                        &resolved.commit,
                        checked_at,
                    )?;
                }
                Err(error) => storage.save_bundle_update_check_failure(
                    &source.source_id,
                    &source.bundle_id,
                    checked_at,
                    &error.to_string(),
                )?,
            }
        }
        lifecycle_lock.recheck(&self.paths)?;
        storage
            .read_initial_scan()?
            .ok_or(ApplicationError::InvalidState("首次扫描状态已经丢失"))
    }

    fn check_editable_local_bundle(
        &self,
        bundle_id: String,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        check_editable_local_bundle_lifecycle(
            &self.paths,
            &lifecycle_lock,
            &mut storage,
            &bundle_id,
            unix_timestamp_millis(),
        )?;
        lifecycle_lock.recheck(&self.paths)?;
        storage
            .read_initial_scan()?
            .ok_or(ApplicationError::InvalidState("首次扫描状态已经丢失"))
    }

    fn open_source_discovery(&self) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        // 打开安装页只读取已保存的目录摘要；网络刷新必须由用户对具体 Source 主动触发。
        let sources = storage.read_source_summaries()?;
        Ok(UiOutcome::SourceDiscovery {
            sources,
            highlighted_source_id: None,
            highlighted_member_path: None,
        })
    }

    fn search_skills_sh(&self, query: String) -> Result<UiOutcome, ApplicationError> {
        let storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let transport = self
            .source_transport
            .as_deref()
            .ok_or(SkillsShError::Network)?;
        let (query, sources) = search_skills_sh(transport, &query)?;
        Ok(UiOutcome::SkillsShSearch { query, sources })
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
                locator: resolved.repository_url.as_str(),
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
            SaveGitHubSourceResult::Saved { source_id } => {
                write_notice_from_storage(&self.paths, lifecycle_lock.root(), &storage)?;
                lifecycle_lock.recheck(&self.paths)?;
                Ok(UiOutcome::SourceDiscovery {
                    sources: storage.read_source_summaries()?,
                    highlighted_source_id: Some(source_id),
                    highlighted_member_path: resolved.member_hint,
                })
            }
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
        write_notice_from_storage(&self.paths, lifecycle_lock.root(), &storage)?;
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

    fn create_editable_local_relink_plan(
        &self,
        source_id: String,
        candidate_path: String,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        let source = storage.read_source_install_source(&source_id)?;
        if source.kind != SourceKind::EditableLocal.as_str() {
            return Err(ApplicationError::InvalidState(
                "只有 Editable Local Source 可以重新指定目录",
            ));
        }
        let identity = SourceFilesystemIdentity {
            device: source
                .filesystem_device
                .ok_or(StorageError::InvalidSourceDefinition)?,
            inode: source
                .filesystem_inode
                .ok_or(StorageError::InvalidSourceDefinition)?,
        };
        let inspected = inspect_editable_local_source(Path::new(&candidate_path), identity)
            .map_err(LifecycleError::from)?;
        if inspected.canonical_path.starts_with(self.paths.data_root()) {
            return Err(ApplicationError::InvalidState(
                "Editable Local Source 不能重新关联到 Central Store 内部",
            ));
        }
        let canonical_path = inspected
            .canonical_path
            .to_str()
            .ok_or(SourceInputError::InvalidEditableLocal)
            .map_err(LifecycleError::from)?;
        if canonical_path == source.locator {
            return Err(ApplicationError::InvalidState(
                "所选目录已经是当前 Editable Local Source 路径",
            ));
        }
        let members = inspected
            .candidates
            .into_iter()
            .map(|candidate| {
                let selectable = candidate.selectable();
                let relative_path = candidate
                    .relative_path
                    .to_str()
                    .ok_or(SourceInputError::InvalidEditableLocal)
                    .map_err(LifecycleError::from)?;
                Ok(EditableLocalRelinkMember {
                    relative_path: relative_path.to_owned(),
                    skill_name: candidate.name,
                    description: candidate.description,
                    selectable,
                    validation_errors: candidate.validation_errors,
                    warnings: candidate.warnings,
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?;
        let now = unix_timestamp_millis();
        let plan_id = uuid::Uuid::new_v4().to_string();
        let plan = storage.create_editable_local_relink_plan(NewEditableLocalRelinkPlan {
            id: &plan_id,
            source: &source,
            candidate_path: canonical_path,
            candidate_display_name: &inspected.display_name,
            candidate_marker: &inspected.marker,
            members: &members,
            created_at: now,
            expires_at: now.saturating_add(SOURCE_REF_PLAN_TTL_MILLIS),
        })?;
        lifecycle_lock.recheck(&self.paths)?;
        Ok(UiOutcome::EditableLocalRelinkPlan { plan })
    }

    fn confirm_editable_local_relink_plan(
        &self,
        plan_id: String,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        let now = unix_timestamp_millis();
        let plan = storage.read_editable_local_relink_plan(&plan_id, now)?;
        let inspected = inspect_editable_local_source(
            Path::new(&plan.public.candidate_path),
            SourceFilesystemIdentity {
                device: plan.expected_device,
                inode: plan.expected_inode,
            },
        )
        .map_err(LifecycleError::from)?;
        if inspected.canonical_path.starts_with(self.paths.data_root()) {
            return Err(ApplicationError::InvalidState(
                "Editable Local Source 不能重新关联到 Central Store 内部",
            ));
        }
        let candidate_path = inspected
            .canonical_path
            .to_str()
            .ok_or(SourceInputError::InvalidEditableLocal)
            .map_err(LifecycleError::from)?;
        let source_id = storage.confirm_editable_local_relink_plan(
            &plan,
            candidate_path,
            &inspected.display_name,
            &inspected.marker,
            now,
        )?;
        lifecycle_lock.recheck(&self.paths)?;
        write_notice_from_storage(&self.paths, lifecycle_lock.root(), &storage)?;
        lifecycle_lock.recheck(&self.paths)?;
        Ok(UiOutcome::SourceDiscovery {
            sources: storage.read_source_summaries()?,
            highlighted_source_id: Some(source_id),
            highlighted_member_path: None,
        })
    }

    fn discard_editable_local_relink_plan(
        &self,
        plan_id: String,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        let source_id = storage.discard_editable_local_relink_plan(&plan_id)?;
        lifecycle_lock.recheck(&self.paths)?;
        Ok(UiOutcome::SourceDiscovery {
            sources: storage.read_source_summaries()?,
            highlighted_source_id: Some(source_id),
            highlighted_member_path: None,
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
        Ok(UiOutcome::InstallPlan { plan })
    }

    fn create_github_install_plan(&self, source_id: String) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        let transport = self
            .source_transport
            .as_deref()
            .ok_or(GithubSourceError::Network)?;
        let plan = create_github_install_plan(
            &self.paths,
            &lifecycle_lock,
            &mut storage,
            transport,
            &source_id,
            unix_timestamp_millis(),
        )?;
        lifecycle_lock.recheck(&self.paths)?;
        Ok(UiOutcome::InstallPlan { plan })
    }

    fn create_bundle_update_plan(&self, bundle_id: String) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        let plan = create_bundle_update_plan(
            &self.paths,
            &lifecycle_lock,
            &mut storage,
            self.source_transport.as_deref(),
            &bundle_id,
            unix_timestamp_millis(),
        )?;
        lifecycle_lock.recheck(&self.paths)?;
        Ok(UiOutcome::InstallPlan { plan })
    }

    fn create_bundle_update_batch_plan(&self) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let plan = create_bundle_update_batch_plan(
            &self.paths,
            &mut storage,
            self.source_transport.as_deref(),
            unix_timestamp_millis(),
        )?;
        Ok(UiOutcome::BundleUpdateBatchPlan { plan })
    }

    fn confirm_bundle_update_batch_plan(
        &self,
        plan_id: String,
        selected_item_ids: Vec<String>,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let result = confirm_bundle_update_batch_plan(
            &self.paths,
            &mut storage,
            &plan_id,
            &selected_item_ids,
            unix_timestamp_millis(),
            self.lifecycle_failpoint,
        )?;
        Ok(UiOutcome::BundleUpdateBatchResult { result })
    }

    fn discard_bundle_update_batch_plan(
        &self,
        plan_id: String,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        discard_bundle_update_batch_plan(&self.paths, &mut storage, &plan_id)?;
        storage
            .read_initial_scan()?
            .ok_or(ApplicationError::InvalidState("首次扫描状态已经丢失"))
    }

    fn acknowledge_bundle_update_batch(
        &self,
        batch_id: String,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        acknowledge_bundle_update_batch(&mut storage, &batch_id)?;
        storage
            .read_initial_scan()?
            .ok_or(ApplicationError::InvalidState("首次扫描状态已经丢失"))
    }

    fn create_bundle_replacement_plan(
        &self,
        bundle_id: String,
        input_path: String,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        let plan = create_bundle_replacement_plan(
            &self.paths,
            &lifecycle_lock,
            &mut storage,
            &bundle_id,
            std::path::Path::new(&input_path),
            unix_timestamp_millis(),
        )?;
        lifecycle_lock.recheck(&self.paths)?;
        Ok(UiOutcome::InstallPlan { plan })
    }

    fn create_archive_install_plan(
        &self,
        input_path: String,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        let plan = create_archive_install_plan(
            &self.paths,
            &lifecycle_lock,
            &mut storage,
            std::path::Path::new(&input_path),
            unix_timestamp_millis(),
        )?;
        lifecycle_lock.recheck(&self.paths)?;
        Ok(UiOutcome::InstallPlan { plan })
    }

    fn create_url_install_plan(&self, url: String) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        let transport = self
            .source_transport
            .as_deref()
            .ok_or_else(|| LifecycleError::SourceInput("Source 下载失败".to_owned()))?;
        let plan = create_url_install_plan(
            &self.paths,
            &lifecycle_lock,
            &mut storage,
            transport,
            &url,
            unix_timestamp_millis(),
        )?;
        lifecycle_lock.recheck(&self.paths)?;
        Ok(UiOutcome::InstallPlan { plan })
    }

    fn create_editable_local_install_plan(
        &self,
        input_path: String,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let lifecycle_lock = acquire_lifecycle_lock(&self.paths)?;
        lifecycle_lock.recheck(&self.paths)?;
        let plan = create_editable_local_install_plan(
            &self.paths,
            &lifecycle_lock,
            &mut storage,
            std::path::Path::new(&input_path),
            unix_timestamp_millis(),
        )?;
        lifecycle_lock.recheck(&self.paths)?;
        Ok(UiOutcome::InstallPlan { plan })
    }

    fn confirm_install_plan(
        &self,
        plan_id: String,
        selected_candidate_ids: Vec<String>,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        confirm_install(
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

    fn discard_install_plan(&self, plan_id: String) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_storage()?;
        let confirmation_started = storage.install_plan_confirmation_has_started(&plan_id)?;
        self.recover_storage(&mut storage)?;
        ensure_onboarding_completed(&storage)?;
        if confirmation_started {
            return Err(StorageError::InstallPlanConsumed.into());
        }
        discard_install_plan(&self.paths, &mut storage, &plan_id)?;
        Ok(UiOutcome::InstallPlanDiscarded)
    }

    fn create_source_association_plan(
        &self,
        bundle_id: String,
        source_id: String,
        member_choices: Vec<SourceMemberMappingChoice>,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let plan = create_source_association_plan(
            &self.paths,
            &mut storage,
            &bundle_id,
            &source_id,
            member_choices,
            unix_timestamp_millis(),
        )?;
        Ok(UiOutcome::SourceAssociationPlan { plan })
    }

    fn confirm_source_association_plan(
        &self,
        plan_id: String,
        content_choices: Vec<MergeContentChoice>,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        confirm_source_association_plan(
            &self.paths,
            &mut storage,
            &plan_id,
            content_choices,
            unix_timestamp_millis(),
            self.lifecycle_failpoint,
        )?;
        storage
            .read_initial_scan()?
            .ok_or(ApplicationError::InvalidState("首次扫描状态已经丢失"))
    }

    fn discard_source_association_plan(
        &self,
        plan_id: String,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        discard_source_association_plan(&self.paths, &mut storage, &plan_id)?;
        Ok(UiOutcome::SourceAssociationPlanDiscarded)
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

    fn create_project_removal_plan(
        &self,
        project_id: String,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let plan = create_project_removal_plan(
            &self.paths,
            &mut storage,
            &project_id,
            unix_timestamp_millis(),
        )?;
        Ok(UiOutcome::RemovalPlan { plan })
    }

    fn create_source_removal_plan(&self, source_id: String) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let plan = create_source_removal_plan(&mut storage, &source_id, unix_timestamp_millis())?;
        Ok(UiOutcome::RemovalPlan { plan })
    }

    fn create_bundle_removal_plan(&self, bundle_id: String) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let plan = create_bundle_removal_plan(
            &self.paths,
            &mut storage,
            &bundle_id,
            unix_timestamp_millis(),
        )?;
        Ok(UiOutcome::RemovalPlan { plan })
    }

    fn create_bundle_mount_removal_plan(
        &self,
        bundle_id: String,
    ) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let plan = create_bundle_mount_removal_plan(
            &self.paths,
            &mut storage,
            &bundle_id,
            unix_timestamp_millis(),
        )?;
        Ok(UiOutcome::RemovalPlan { plan })
    }

    fn confirm_removal_plan(&self, plan_id: String) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        let kind = confirm_removal_plan(
            &self.paths,
            &mut storage,
            &plan_id,
            unix_timestamp_millis(),
            self.lifecycle_failpoint,
        )?;
        match kind {
            crate::domain::RemovalKind::Source => Ok(UiOutcome::SourceDiscovery {
                sources: storage.read_source_summaries()?,
                highlighted_source_id: None,
                highlighted_member_path: None,
            }),
            crate::domain::RemovalKind::Project
            | crate::domain::RemovalKind::Bundle
            | crate::domain::RemovalKind::BundleMounts => storage
                .read_initial_scan()?
                .ok_or(ApplicationError::InvalidState("首次扫描状态已经丢失")),
        }
    }

    fn discard_removal_plan(&self, plan_id: String) -> Result<UiOutcome, ApplicationError> {
        let mut storage = self.open_recovered_storage()?;
        ensure_onboarding_completed(&storage)?;
        match discard_removal_plan(&mut storage, &plan_id)? {
            crate::domain::RemovalKind::Source => Ok(UiOutcome::SourceDiscovery {
                sources: storage.read_source_summaries()?,
                highlighted_source_id: None,
                highlighted_member_path: None,
            }),
            crate::domain::RemovalKind::Project
            | crate::domain::RemovalKind::Bundle
            | crate::domain::RemovalKind::BundleMounts => storage
                .read_initial_scan()?
                .ok_or(ApplicationError::InvalidState("首次扫描状态已经丢失")),
        }
    }

    fn open_recovered_storage(&self) -> Result<Storage, ApplicationError> {
        self.open_recovered_storage_with_notice()
            .map(|(storage, _)| storage)
    }

    fn open_recovered_storage_with_notice(&self) -> Result<(Storage, bool), ApplicationError> {
        let mut storage = self.open_storage()?;
        let recovered_interrupted_operation = self.recover_storage(&mut storage)?;
        Ok((storage, recovered_interrupted_operation))
    }

    fn open_storage(&self) -> Result<Storage, ApplicationError> {
        let storage = Storage::open(self.paths.data_root(), &self.paths.database())?;
        ensure_central_store_layout(&self.paths)?;
        Ok(storage)
    }

    fn recover_storage(&self, storage: &mut Storage) -> Result<bool, ApplicationError> {
        let recovered_interrupted_operation = storage.has_pending_recovery_work()?;
        recover_pending_transactions(&self.paths, storage, unix_timestamp_millis())?;
        recover_pending_source_association_transactions(
            &self.paths,
            storage,
            unix_timestamp_millis(),
            self.lifecycle_failpoint,
        )?;
        recover_pending_mount_transactions(&self.paths, storage, unix_timestamp_millis())?;
        recover_pending_batch_mount_transactions(&self.paths, storage, unix_timestamp_millis())?;
        recover_pending_takeover_transactions(
            &self.paths,
            storage,
            unix_timestamp_millis(),
            self.lifecycle_failpoint,
        )?;
        recover_pending_removals(
            &self.paths,
            storage,
            unix_timestamp_millis(),
            self.lifecycle_failpoint,
        )?;
        recover_running_bundle_update_batch(
            &self.paths,
            storage,
            unix_timestamp_millis(),
            self.lifecycle_failpoint,
        )?;
        Ok(recovered_interrupted_operation)
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
    fn finder_entry_is_fixed_to_the_application_data_root() {
        let sandbox = tempdir().expect("应创建隔离测试目录");
        let data_root = sandbox.path().join("data");
        let application = SkillYardApplication::new(
            ApplicationPaths::for_home(data_root.clone(), sandbox.path().join("home")),
            PlatformInfo::supported_for_test(),
        );

        assert_eq!(application.central_store_path(), data_root);
    }

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
