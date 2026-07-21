use std::path::{Path, PathBuf};

use crate::domain::{ScanRootKey, SupportedAppId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SupportedAppPathConfig {
    pub id: SupportedAppId,
    pub display_name: &'static str,
    pub global_root: PathBuf,
    pub project_relative_root: PathBuf,
    pub detection_root: PathBuf,
    pub root_key: ScanRootKey,
    pub project_root_key: ScanRootKey,
}

/// 生产路径固定，测试只能通过构造隔离实例替换根目录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationPaths {
    data_root: PathBuf,
    home: PathBuf,
}

impl ApplicationPaths {
    pub fn for_home(data_root: PathBuf, home: PathBuf) -> Self {
        Self { data_root, home }
    }

    pub(crate) fn data_root(&self) -> &Path {
        &self.data_root
    }

    pub(crate) fn database(&self) -> PathBuf {
        self.data_root.join("skillyard.sqlite3")
    }

    pub(crate) fn bundles_root(&self) -> PathBuf {
        self.data_root.join("bundles")
    }

    pub(crate) fn staging_root(&self) -> PathBuf {
        self.data_root.join("staging")
    }

    pub(crate) fn journals_root(&self) -> PathBuf {
        self.data_root.join("journals")
    }

    pub(crate) fn central_store_notice(&self) -> PathBuf {
        self.data_root.join("SKILLYARD-INFO.md")
    }

    pub(crate) fn bundle_directory(&self, bundle_id: &str) -> PathBuf {
        self.bundles_root().join(bundle_id)
    }

    #[allow(dead_code)]
    pub(crate) fn home(&self) -> &Path {
        &self.home
    }

    pub(crate) fn supported_apps(&self) -> Vec<SupportedAppPathConfig> {
        vec![
            SupportedAppPathConfig {
                id: SupportedAppId::Codex,
                display_name: "Codex",
                global_root: self.home.join(".codex/skills"),
                project_relative_root: PathBuf::from(".codex/skills"),
                detection_root: self.home.join(".codex"),
                root_key: ScanRootKey::CodexGlobal,
                project_root_key: ScanRootKey::CodexProject,
            },
            SupportedAppPathConfig {
                id: SupportedAppId::ClaudeCode,
                display_name: "Claude Code",
                global_root: self.home.join(".claude/skills"),
                project_relative_root: PathBuf::from(".claude/skills"),
                detection_root: self.home.join(".claude"),
                root_key: ScanRootKey::ClaudeCodeGlobal,
                project_root_key: ScanRootKey::ClaudeCodeProject,
            },
            SupportedAppPathConfig {
                id: SupportedAppId::GitHubCopilot,
                display_name: "GitHub Copilot",
                global_root: self.home.join(".copilot/skills"),
                project_relative_root: PathBuf::from(".github/skills"),
                detection_root: self.home.join(".copilot"),
                root_key: ScanRootKey::GitHubCopilotGlobal,
                project_root_key: ScanRootKey::GitHubCopilotProject,
            },
        ]
    }

    pub(crate) fn shared_read_only_root(&self) -> PathBuf {
        self.home.join(".agents/skills")
    }
}
