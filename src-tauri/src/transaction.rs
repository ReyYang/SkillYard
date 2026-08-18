//! Filesystem Transaction Journal 的共享内核。
//!
//! 六个生命周期引擎（Install、Mount、Batch Mount、Takeover、Removal、Source
//! Association）使用同一套 journal 文件协议：受限序列化、原子写入、受限读取、
//! 幂等清理。内核只承载这套字节级协议；各引擎的 phase 枚举、journal 结构体、
//! 恢复决策树与大小限制常量仍由引擎自己持有，内核错误由 adapter 映射回引擎
//! 自己的错误词汇——同一「超限」条件在不同引擎、不同读写阶段的对外语义不同，
//! 因此映射不进入内核。

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::lifecycle::{
    LifecycleError, open_managed_directory_from_root, open_regular_file_at, unlink_at,
    write_atomic_at,
};
use crate::paths::ApplicationPaths;

/// Journal 字节级协议在内核层只区分三种失败。
#[derive(Debug)]
pub(crate) enum JournalIoError {
    /// 序列化或读取结果超过引擎声明的大小上限。
    TooLarge { actual: usize, limit: usize },
    /// Journal 字节无法解析为引擎声明的 JSON 结构。
    InvalidJson(serde_json::Error),
    /// 共享文件系统原语失败（IO、Central Store 安全校验等）。
    Lifecycle(LifecycleError),
}

impl JournalIoError {
    fn io(action: &'static str, path: &Path, source: io::Error) -> Self {
        Self::Lifecycle(LifecycleError::Io {
            action,
            path: path.display().to_string(),
            source,
        })
    }
}

/// 引擎约定的 journal 文件名：`{prefix}{transaction_id}.json`。
pub(crate) fn journal_file_name(prefix: &str, transaction_id: &str) -> OsString {
    OsString::from(format!("{prefix}{transaction_id}.json"))
}

pub(crate) fn journal_path(paths: &ApplicationPaths, name: &OsStr) -> PathBuf {
    paths.journals_root().join(name)
}

/// 序列化并校验大小上限；恢复端用同一上限读取，写入端必须先证明能写。
pub(crate) fn serialize_journal(
    journal: &impl Serialize,
    max_bytes: usize,
    pretty: bool,
) -> Result<Vec<u8>, JournalIoError> {
    let bytes = if pretty {
        serde_json::to_vec_pretty(journal)
    } else {
        serde_json::to_vec(journal)
    }
    .map_err(JournalIoError::InvalidJson)?;
    ensure_journal_bytes_fit(bytes.len(), max_bytes)?;
    Ok(bytes)
}

fn ensure_journal_bytes_fit(actual: usize, limit: usize) -> Result<(), JournalIoError> {
    if actual > limit {
        Err(JournalIoError::TooLarge { actual, limit })
    } else {
        Ok(())
    }
}

/// 在 journals 目录内原子写入 journal（临时文件 + sync + rename + 目录 sync）。
pub(crate) fn write_journal(
    paths: &ApplicationPaths,
    managed_root: &File,
    name: &OsStr,
    journal: &impl Serialize,
    max_bytes: usize,
    pretty: bool,
) -> Result<(), JournalIoError> {
    let bytes = serialize_journal(journal, max_bytes, pretty)?;
    let journals = open_managed_directory_from_root(paths, managed_root, &paths.journals_root())
        .map_err(JournalIoError::Lifecycle)?;
    write_atomic_at(&journals, name, &journal_path(paths, name), &bytes)
        .map_err(JournalIoError::Lifecycle)
}

/// 受限读取：文件元数据与内容都不得超过 `max_bytes`。
/// `check_action` / `read_action` 保留各引擎既有的中文错误语境。
pub(crate) fn read_journal<T: DeserializeOwned>(
    journals: &File,
    name: &OsStr,
    path: &Path,
    max_bytes: usize,
    check_action: &'static str,
    read_action: &'static str,
) -> Result<T, JournalIoError> {
    let mut file =
        open_regular_file_at(journals, name, path, false).map_err(JournalIoError::Lifecycle)?;
    let metadata = file
        .metadata()
        .map_err(|source| JournalIoError::io(check_action, path, source))?;
    if metadata.len() > max_bytes as u64 {
        return Err(JournalIoError::TooLarge {
            actual: metadata.len() as usize,
            limit: max_bytes,
        });
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize + 1);
    Read::by_ref(&mut file)
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| JournalIoError::io(read_action, path, source))?;
    ensure_journal_bytes_fit(bytes.len(), max_bytes)?;
    serde_json::from_slice(&bytes).map_err(JournalIoError::InvalidJson)
}

/// 幂等清理：journal 缺失不算失败，删除后同步 journals 目录。
/// `sync_when_missing` 保留各引擎的历史语义：Removal 与 SourceAssociation
/// 在重构前即使文件缺失也会同步目录，Install/Mount/Takeover 则不会。
pub(crate) fn remove_journal(
    paths: &ApplicationPaths,
    managed_root: &File,
    name: &OsStr,
    remove_action: &'static str,
    sync_action: &'static str,
    sync_when_missing: bool,
) -> Result<(), JournalIoError> {
    let journals = open_managed_directory_from_root(paths, managed_root, &paths.journals_root())
        .map_err(JournalIoError::Lifecycle)?;
    match unlink_at(&journals, name, false) {
        Ok(()) => journals
            .sync_all()
            .map_err(|source| JournalIoError::io(sync_action, &paths.journals_root(), source)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            if sync_when_missing {
                journals.sync_all().map_err(|source| {
                    JournalIoError::io(sync_action, &paths.journals_root(), source)
                })
            } else {
                Ok(())
            }
        }
        Err(source) => Err(JournalIoError::io(
            remove_action,
            &journal_path(paths, name),
            source,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::fs;
    use tempfile::tempdir;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct TestJournal {
        version: u32,
        transaction_id: String,
        note: String,
    }

    fn test_paths(temp: &tempfile::TempDir) -> (ApplicationPaths, File) {
        let data_root = temp.path().join("data");
        let paths = ApplicationPaths::for_home(data_root, temp.path().join("home"));
        fs::create_dir_all(paths.journals_root()).expect("应创建 journals 目录");
        let root = File::open(paths.data_root()).expect("应打开受管根目录");
        (paths, root)
    }

    fn test_journal(note: &str) -> TestJournal {
        TestJournal {
            version: 1,
            transaction_id: "txn-1".to_owned(),
            note: note.to_owned(),
        }
    }

    #[test]
    fn written_journal_round_trips_through_restricted_read() {
        let temp = tempdir().expect("应创建临时目录");
        let (paths, root) = test_paths(&temp);
        let name = journal_file_name("", "txn-1");
        let journal = test_journal("候选已发布");

        write_journal(&paths, &root, &name, &journal, 1_048_576, true).expect("应写入 journal");
        let journals = File::open(paths.journals_root()).expect("应打开 journals 目录");
        let restored: TestJournal = read_journal(
            &journals,
            &name,
            &journal_path(&paths, &name),
            1_048_576,
            "检查 Journal",
            "读取 Journal",
        )
        .expect("应读回 journal");

        assert_eq!(restored, journal);
    }

    #[test]
    fn oversized_write_is_rejected_before_any_file_appears() {
        let temp = tempdir().expect("应创建临时目录");
        let (paths, root) = test_paths(&temp);
        let name = journal_file_name("", "txn-1");
        let journal = test_journal(&"长".repeat(64));

        let result = write_journal(&paths, &root, &name, &journal, 16, false);

        assert!(
            matches!(
                result,
                Err(JournalIoError::TooLarge {
                    actual,
                    limit: 16
                }) if actual > 16
            ),
            "超限写入应在落盘前被拒绝：{result:?}"
        );
        assert!(
            !journal_path(&paths, &name).exists(),
            "超限 journal 不应留下文件"
        );
    }

    #[test]
    fn oversized_file_is_rejected_by_read_limit() {
        let temp = tempdir().expect("应创建临时目录");
        let (paths, root) = test_paths(&temp);
        let name = journal_file_name("", "txn-1");
        write_journal(&paths, &root, &name, &test_journal("合法"), 1_048_576, true)
            .expect("应写入合法 journal");
        let journals = File::open(paths.journals_root()).expect("应打开 journals 目录");

        let result = read_journal::<TestJournal>(
            &journals,
            &name,
            &journal_path(&paths, &name),
            16,
            "检查 Journal",
            "读取 Journal",
        );

        assert!(
            matches!(result, Err(JournalIoError::TooLarge { limit: 16, .. })),
            "读取端应使用与写入端相同的上限：{result:?}"
        );
    }

    #[test]
    fn invalid_json_is_reported_without_confusing_it_with_size() {
        let temp = tempdir().expect("应创建临时目录");
        let (paths, _root) = test_paths(&temp);
        let name = journal_file_name("", "txn-1");
        fs::write(journal_path(&paths, &name), b"{ not json").expect("应写入损坏 journal");
        let journals = File::open(paths.journals_root()).expect("应打开 journals 目录");

        let result = read_journal::<TestJournal>(
            &journals,
            &name,
            &journal_path(&paths, &name),
            1_048_576,
            "检查 Journal",
            "读取 Journal",
        );

        assert!(
            matches!(result, Err(JournalIoError::InvalidJson(_))),
            "损坏 journal 应报告为解析失败：{result:?}"
        );
    }

    #[test]
    fn remove_is_idempotent_for_missing_and_existing_journals() {
        let temp = tempdir().expect("应创建临时目录");
        let (paths, root) = test_paths(&temp);
        let name = journal_file_name("", "txn-1");

        remove_journal(
            &paths,
            &root,
            &name,
            "清理事务 Journal",
            "同步 journals 目录",
            false,
        )
        .expect("缺失的 journal 应视为已清理");

        write_journal(
            &paths,
            &root,
            &name,
            &test_journal("待清理"),
            1_048_576,
            false,
        )
        .expect("应写入 journal");
        remove_journal(
            &paths,
            &root,
            &name,
            "清理事务 Journal",
            "同步 journals 目录",
            false,
        )
        .expect("应清理已存在的 journal");

        assert!(!journal_path(&paths, &name).exists(), "journal 应被删除");
    }

    #[test]
    fn remove_with_sync_when_missing_also_accepts_absent_journal() {
        let temp = tempdir().expect("应创建临时目录");
        let (paths, root) = test_paths(&temp);
        let name = journal_file_name("", "txn-1");

        remove_journal(
            &paths,
            &root,
            &name,
            "清理事务 Journal",
            "同步 journals 目录",
            true,
        )
        .expect("缺失的 journal 在目录同步语义下也应视为已清理");
    }

    #[test]
    fn compact_and_pretty_serializations_both_parse() {
        let journal = test_journal("格式");
        let pretty = serialize_journal(&journal, 1_048_576, true).expect("应序列化 pretty");
        let compact = serialize_journal(&journal, 1_048_576, false).expect("应序列化 compact");
        assert!(pretty.len() > compact.len());
        let parsed: TestJournal =
            serde_json::from_slice(&compact).expect("compact 与 pretty 应可互相解析");
        assert_eq!(parsed, journal);
    }
}
