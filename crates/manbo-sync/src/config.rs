//! `[sync]` 配置分节。挂进 [`manbo_platform::Config`](../manbo_platform/struct.Config.html)，
//! 模板与缺省值约定同 `[predict]`：全部默认值收在本文件的 `Default` 里，模板只是把它们写出来并加注释。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::SyncError;

/// 同步后端。两种共用同一套合并逻辑，只是传输不同。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    /// WebDAV（坚果云 / Nextcloud / NAS），凭 Basic 认证上传下载。
    #[default]
    Webdav,

    /// 本地同步文件夹：交给 iCloud Drive / Dropbox / Syncthing / 坚果云同步盘传输，
    /// 输入法只读写目录里的文件；也是离线测试与 CLI 的后端。
    Folder,
}

impl BackendKind {
    /// 模板与日志里的写法。
    pub fn key(self) -> &'static str {
        match self {
            Self::Webdav => "webdav",
            Self::Folder => "folder",
        }
    }
}

/// 学习数据同步配置。默认**关闭**（opt-in）：打开意味着学习表与配置会传到用户自己选的后端。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncConfig {
    /// 是否启用同步。
    pub enabled: bool,

    /// 用哪种后端。
    pub backend: BackendKind,

    /// WebDAV 地址，到要建（或已建）的集合为止，如 `https://dav.jianguoyun.com/dav/manbo`。
    pub url: String,

    /// WebDAV 用户名（坚果云是登录邮箱，密码用应用密码）。
    pub username: String,

    /// WebDAV 密码。留空则读 `password_env` 指定的环境变量。
    pub password: Option<String>,

    /// 存放 WebDAV 密码的环境变量名。
    pub password_env: String,

    /// `backend = "folder"` 时的同步目录。
    pub folder: Option<PathBuf>,

    /// 运行中自动同步的间隔（分钟）；退出与启动各固定跑一轮全量，不受它管。
    pub interval_minutes: u64,

    /// 启动时同步的限时（毫秒）：超时不等，先起输入法，合并线程继续写完，下一轮收敛。
    pub startup_timeout_ms: u64,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: BackendKind::Webdav,
            // 缺省给坚果云的示例地址（照 predict 缺省 DeepSeek 的约定）：启用 + 用户名 + 应用密码即可用
            url: "https://dav.jianguoyun.com/dav/manbo".to_owned(),
            username: String::new(),
            password: None,
            password_env: "MANBO_SYNC_PASSWORD".to_owned(),
            folder: None,
            interval_minutes: 15,
            startup_timeout_ms: 3000,
        }
    }
}

impl SyncConfig {
    /// 配置里的密码优先，其次环境变量；两边都没有返回 `None`（照 [`predict 的 resolve_api_key`] 的约定）。
    ///
    /// [`predict 的 resolve_api_key`]: manbo_predict::PredictConfig::resolve_api_key
    pub fn resolve_password(&self) -> Option<String> {
        self.password
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .or_else(|| std::env::var(&self.password_env).ok())
            .filter(|p| !p.trim().is_empty())
    }

    /// 启用时的必填项检查：WebDAV 要地址，文件夹后端要目录。缺了在起服务前就报错，壳降级不同步。
    pub fn validate(&self) -> Result<(), SyncError> {
        match self.backend {
            BackendKind::Webdav if self.url.trim().is_empty() => Err(SyncError::MissingUrl),
            BackendKind::Folder if self.folder.as_ref().is_none_or(|f| f.as_os_str().is_empty()) => {
                Err(SyncError::MissingFolder)
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_disabled_webdav() {
        let config = SyncConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.backend, BackendKind::Webdav);
        assert_eq!(config.password_env, "MANBO_SYNC_PASSWORD");
        assert_eq!(config.interval_minutes, 15);
        assert_eq!(config.startup_timeout_ms, 3000);
        // 缺省给了坚果云示例地址：enabled 一开、补上用户名与应用密码即可用，不必再改 url
        config.validate().unwrap();
    }

    #[test]
    fn parses_a_webdav_section() {
        // platform 的 Config 挂法：外层 wrapper 带 `sync` 键
        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(default)]
            sync: SyncConfig,
        }
        let wrapper: Wrapper =
            toml::from_str("[sync]\nurl = \"https://dav.jianguoyun.com/dav/qj\"\nusername = \"a@b.c\"\n")
                .unwrap();
        let config = wrapper.sync;
        assert_eq!(config.backend, BackendKind::Webdav);
        assert_eq!(config.url, "https://dav.jianguoyun.com/dav/qj");
        assert_eq!(config.interval_minutes, 15);
    }

    #[test]
    fn parses_a_folder_section() {
        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(default)]
            sync: SyncConfig,
        }
        let wrapper: Wrapper =
            toml::from_str("[sync]\nbackend = \"folder\"\nfolder = \"/sync/manbo\"\n").unwrap();
        let config = wrapper.sync;
        assert_eq!(config.backend, BackendKind::Folder);
        assert_eq!(config.folder.as_deref(), Some(std::path::Path::new("/sync/manbo")));
    }

    #[test]
    fn password_falls_back_to_the_env_var() {
        let mut config = SyncConfig::default();
        assert_eq!(config.resolve_password(), None);
        config.password = Some(" app-pass ".to_owned());
        assert_eq!(config.resolve_password().as_deref(), Some("app-pass"));
        config.password = None;
        // 环境变量是进程全局的：单测里用独立变量名，改动仅影响本测试进程
        // SAFETY：单线程写独立变量名，无并发读
        unsafe {
            std::env::set_var("MANBO_SYNC_PASSWORD_TEST", "env-pass");
        }
        config.password_env = "MANBO_SYNC_PASSWORD_TEST".to_owned();
        assert_eq!(config.resolve_password().as_deref(), Some("env-pass"));
        unsafe {
            std::env::remove_var("MANBO_SYNC_PASSWORD_TEST");
        }
    }

    #[test]
    fn validate_requires_url_or_folder() {
        let mut config = SyncConfig::default();
        config.enabled = true;
        config.url = String::new();
        assert!(matches!(config.validate(), Err(SyncError::MissingUrl)));
        config.backend = BackendKind::Folder;
        assert!(matches!(config.validate(), Err(SyncError::MissingFolder)));
        config.folder = Some(std::env::temp_dir());
        config.validate().unwrap();
    }
}
