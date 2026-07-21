use std::path::{Path, PathBuf};

use crate::domain::SupportedAppId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SupportedAppPathConfig {
    pub id: SupportedAppId,
    pub display_name: &'static str,
    pub global_root: PathBuf,
    pub detection_root: PathBuf,
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
                detection_root: self.home.join(".codex"),
            },
            SupportedAppPathConfig {
                id: SupportedAppId::ClaudeCode,
                display_name: "Claude Code",
                global_root: self.home.join(".claude/skills"),
                detection_root: self.home.join(".claude"),
            },
            SupportedAppPathConfig {
                id: SupportedAppId::GitHubCopilot,
                display_name: "GitHub Copilot",
                global_root: self.home.join(".copilot/skills"),
                detection_root: self.home.join(".copilot"),
            },
        ]
    }

    pub(crate) fn shared_read_only_root(&self) -> PathBuf {
        self.home.join(".agents/skills")
    }
}
