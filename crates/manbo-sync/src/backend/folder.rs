//! 本地文件夹后端：把一个目录当「云」，同一套合并逻辑写进去，交给 iCloud Drive /
//! Dropbox / Syncthing / 坚果云同步盘去传输。也是集成测试与 CLI 的后端。
//!
//! 版本号 = `"{mtime 毫秒}-{字节数}"`：同进程内可靠，跨设备的并发写靠同步工具解决，
//! 这里只保证「谁后写谁可见」不保证并发安全——文件夹语义本来如此。

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use manbo_core::storage::write_atomic;

use crate::backend::{GetOutcome, RemoteBlob, SyncBackend};
use crate::error::SyncError;

pub(super) struct FolderBackend {
    /// 同步根目录。
    root: PathBuf,
}

impl FolderBackend {
    pub(super) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// 文件的版本号：mtime 毫秒 + 字节数。不存在返回 `None`。
    fn tag_of(path: &Path) -> std::io::Result<Option<String>> {
        match std::fs::metadata(path) {
            Ok(metadata) => Ok(Some(Self::tag_from(&metadata))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn tag_from(metadata: &std::fs::Metadata) -> String {
        let mtime_ms = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_millis());
        format!("{mtime_ms}-{}", metadata.len())
    }
}

impl SyncBackend for FolderBackend {
    fn ensure_root(&mut self) -> Result<(), SyncError> {
        std::fs::create_dir_all(&self.root).map_err(|source| SyncError::Io {
            path: self.root.clone(),
            source,
        })
    }

    fn get(&mut self, name: &str, if_none_match: Option<&str>) -> Result<GetOutcome, SyncError> {
        let path = self.path(name);
        let Some(tag) = Self::tag_of(&path).map_err(|source| SyncError::Io {
            path: path.clone(),
            source,
        })?
        else {
            return Ok(GetOutcome::Missing);
        };
        if if_none_match == Some(tag.as_str()) {
            return Ok(GetOutcome::Unchanged);
        }
        let data = std::fs::read(&path).map_err(|source| SyncError::Io {
            path: path.clone(),
            source,
        })?;
        let mtime_ms = std::fs::metadata(&path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis());
        Ok(GetOutcome::Fresh(RemoteBlob {
            data,
            tag,
            mtime_ms,
        }))
    }

    fn put(&mut self, name: &str, data: &[u8], expect: Option<&str>) -> Result<String, SyncError> {
        let path = self.path(name);
        // 乐观并发：远端现状与我以为的不一致就让 cycle 重取重并
        let current = Self::tag_of(&path).map_err(|source| SyncError::Io {
            path: path.clone(),
            source,
        })?;
        if current.as_deref() != expect {
            return Err(SyncError::Conflict(name.to_owned()));
        }
        let bytes = data.to_vec();
        write_atomic(&path, |writer| std::io::Write::write_all(writer, &bytes)).map_err(
            |source| SyncError::Io {
                path: path.clone(),
                source,
            },
        )?;
        Self::tag_of(&path)
            .map_err(|source| SyncError::Io {
                path: path.clone(),
                source,
            })?
            .ok_or_else(|| SyncError::Io {
                path: path.clone(),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "刚写入的文件不见了"),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(test: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("manbo-sync-folder-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_then_fresh_then_unchanged() {
        let mut backend = FolderBackend::new(scratch("lifecycle"));
        backend.ensure_root().unwrap();
        assert_eq!(backend.get("user.tsv", None).unwrap(), GetOutcome::Missing);
        let tag = backend.put("user.tsv", b"weibo\t10\n", None).unwrap();
        let outcome = backend.get("user.tsv", Some(&tag)).unwrap();
        assert_eq!(outcome, GetOutcome::Unchanged);
        let fresh = backend.get("user.tsv", None).unwrap();
        let GetOutcome::Fresh(blob) = fresh else {
            panic!("应当是 Fresh");
        };
        assert_eq!(blob.data, b"weibo\t10\n");
        assert_eq!(blob.tag, tag);
    }

    #[test]
    fn put_with_stale_expect_conflicts() {
        let mut backend = FolderBackend::new(scratch("conflict"));
        backend.ensure_root().unwrap();
        let tag = backend.put("user.tsv", b"old\n", None).unwrap();
        assert!(matches!(
            backend.put("user.tsv", b"new\n", Some("别的版本")),
            Err(SyncError::Conflict(_))
        ));
        backend.put("user.tsv", b"new\n", Some(&tag)).unwrap();
        assert_eq!(std::fs::read(backend.path("user.tsv")).unwrap(), b"new\n");
    }
}
