//! WebDAV 后端：坚果云 / Nextcloud / NAS。只用到三个动词——MKCOL 建目录、GET 取、PUT 存，
//! 全程不做 PROPFIND（不引 XML 解析）；并发控制靠 ETag（`If-None-Match` / `If-Match`）。
//!
//! 与云联想同栈：reqwest + native-tls，系统证书库；异步客户端跑在 worker 线程自己的
//! current_thread runtime 上（照 [`manbo_predict`] 的 worker 模式）。

use std::time::Duration;

use reqwest::header::{self, HeaderMap, HeaderValue};
use reqwest::{Client, Method, StatusCode};

use crate::backend::{GetOutcome, RemoteBlob, SyncBackend};
use crate::config::SyncConfig;
use crate::error::SyncError;

/// 单请求超时。启动全量同步整体还有壳的限时兜底，这里管的是单个请求别吊死。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// 远端没有 ETag 头时的版本号兜底：字节数 + Last-Modified 原文（文件没变它就不变）。
fn tag_from_headers(headers: &HeaderMap, data_len: usize) -> String {
    if let Some(etag) = headers.get(header::ETAG).and_then(|value| value.to_str().ok()) {
        return etag.trim().to_owned();
    }
    let last_modified = headers
        .get(header::LAST_MODIFIED)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    format!("{data_len}-{last_modified}")
}

/// MKCOL 的答复算不算通过：2xx 是建成了，405 是标准里说的「这个集合已经有了」，**403 也算已有**。
///
/// 403 必须放行，否则坚果云用不了：它对自己固定根路径 `/dav` 的 MKCOL 恒回 403（2026-09-15 实测，
/// 同一凭据下 `/dav/manbo` 的 MKCOL 201、PUT 201、GET 200 都正常），而 `ensure_root` 是逐段建的
/// 第一段就是 `/dav`——只放行 405 会让每一轮同步都在第一段放弃，远端永远空着（复盘见
/// `docs/design/sync.md`「后端」）。凭据不对是 401，真的没权限会在随后的 GET / PUT 上报出来，
/// 两者都不会被这里吞掉。
fn mkcol_is_fine(status: StatusCode) -> bool {
    status.is_success() || status == StatusCode::METHOD_NOT_ALLOWED || status == StatusCode::FORBIDDEN
}

pub(super) struct WebdavBackend {
    /// worker 线程私有的 current_thread runtime。
    runtime: tokio::runtime::Runtime,

    /// HTTP 客户端（带超时）。
    client: Client,

    /// MKCOL 动词（reqwest 的 `Method` 不预置 WebDAV 扩展动词）。
    mkcol: Method,

    /// 去掉尾部斜杠的根集合地址，如 `https://dav.jianguoyun.com/dav/manbo`。
    base_url: String,

    /// 根集合的 scheme://host 前缀（逐段 MKCOL 时拼累积地址用）。
    scheme_host: String,

    /// Basic 认证；用户名留空就不带（匿名可达的 NAS）。
    username: Option<String>,
    password: Option<String>,
}

impl WebdavBackend {
    pub(super) fn new(config: &SyncConfig) -> Result<Self, SyncError> {
        let base_url = config.url.trim().trim_end_matches('/').to_owned();
        if base_url.is_empty() {
            return Err(SyncError::MissingUrl);
        }
        let client = Client::builder().timeout(REQUEST_TIMEOUT).build()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(SyncError::Runtime)?;
        // scheme://host 之前的部分原样留下，路径逐段建集合
        let scheme_host = match base_url.split_once("://") {
            Some((scheme, rest)) => match rest.split_once('/') {
                Some((host, _)) => format!("{scheme}://{host}"),
                None => base_url.clone(),
            },
            None => return Err(SyncError::MissingUrl),
        };
        let username = config.username.trim();
        Ok(Self {
            runtime,
            client,
            mkcol: Method::from_bytes(b"MKCOL").expect("MKCOL 是合法动词"),
            base_url,
            scheme_host,
            username: (!username.is_empty()).then(|| username.to_owned()),
            password: config.resolve_password(),
        })
    }

    fn file_url(&self, name: &str) -> String {
        format!("{}/{name}", self.base_url)
    }

    /// 给请求带上 Basic 认证（配置了用户名才带）。
    fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match (&self.username, &self.password) {
            (Some(username), password) => request.basic_auth(username, password.clone()),
            (None, _) => request,
        }
    }

    async fn ensure_root_async(&self) -> Result<(), SyncError> {
        // 逐段累积 MKCOL：`/dav/manbo` 先建 `/dav` 再建 `/manbo`；「已经有了」按 [`mkcol_is_fine`] 判
        let Some(path) = self.base_url.strip_prefix(&self.scheme_host) else {
            return Ok(());
        };
        let mut cumulative = self.scheme_host.clone();
        for segment in path.split('/').filter(|segment| !segment.is_empty()) {
            cumulative.push('/');
            cumulative.push_str(segment);
            let response = self
                .authorize(self.client.request(self.mkcol.clone(), &cumulative))
                .send()
                .await?;
            let status = response.status();
            if !mkcol_is_fine(status) {
                return Err(SyncError::HttpStatus(status.as_u16()));
            }
        }
        Ok(())
    }

    async fn get_async(&self, name: &str, if_none_match: Option<&str>) -> Result<GetOutcome, SyncError> {
        let mut request = self.authorize(self.client.get(self.file_url(name)));
        if let Some(tag) = if_none_match {
            request = request.header(header::IF_NONE_MATCH, HeaderValue::from_str(tag).map_err(
                |_| SyncError::HttpStatus(0), // 版本号里混进了非法头字符：当状态异常处理
            )?);
        }
        let response = request.send().await?;
        let status = response.status();
        if status == StatusCode::NOT_MODIFIED {
            return Ok(GetOutcome::Unchanged);
        }
        if status == StatusCode::NOT_FOUND {
            return Ok(GetOutcome::Missing);
        }
        if !status.is_success() {
            return Err(SyncError::HttpStatus(status.as_u16()));
        }
        let headers = response.headers().clone();
        let data = response.bytes().await?;
        Ok(GetOutcome::Fresh(RemoteBlob {
            tag: tag_from_headers(&headers, data.len()),
            mtime_ms: None,
            data: data.to_vec(),
        }))
    }

    async fn put_async(&self, name: &str, data: &[u8], expect: Option<&str>) -> Result<String, SyncError> {
        let mut request = self.authorize(self.client.put(self.file_url(name)));
        request = match expect {
            Some(tag) => {
                let value = HeaderValue::from_str(tag).map_err(|_| SyncError::HttpStatus(0))?;
                request.header(header::IF_MATCH, value)
            }
            // 首次上传要求远端确实没有这个文件，避免盖掉别人的
            None => request.header(header::IF_NONE_MATCH, HeaderValue::from_static("*")),
        };
        let response = request.body(data.to_vec()).send().await?;
        let status = response.status();
        if status == StatusCode::PRECONDITION_FAILED {
            return Err(SyncError::Conflict(name.to_owned()));
        }
        if !status.is_success() {
            return Err(SyncError::HttpStatus(status.as_u16()));
        }
        // 服务器存完不回 ETag 的话再取一次，拿到版本号下次才能做 304 / If-Match
        match response.headers().get(header::ETAG).and_then(|value| value.to_str().ok()) {
            Some(etag) => Ok(etag.trim().to_owned()),
            None => match self.get_async(name, None).await? {
                GetOutcome::Fresh(blob) => Ok(blob.tag),
                _ => Err(SyncError::HttpStatus(0)),
            },
        }
    }
}

impl SyncBackend for WebdavBackend {
    fn ensure_root(&mut self) -> Result<(), SyncError> {
        self.runtime.block_on(self.ensure_root_async())
    }

    fn get(&mut self, name: &str, if_none_match: Option<&str>) -> Result<GetOutcome, SyncError> {
        self.runtime.block_on(self.get_async(name, if_none_match))
    }

    fn put(&mut self, name: &str, data: &[u8], expect: Option<&str>) -> Result<String, SyncError> {
        self.runtime.block_on(self.put_async(name, data, expect))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BackendKind;

    #[test]
    fn requires_a_url() {
        let mut config = SyncConfig::default();
        config.backend = BackendKind::Webdav;
        config.url = "  ".to_owned();
        assert!(matches!(WebdavBackend::new(&config), Err(SyncError::MissingUrl)));
    }

    #[test]
    fn splits_the_scheme_host_for_mkcol() {
        let mut config = SyncConfig::default();
        config.url = "https://dav.jianguoyun.com/dav/manbo/".to_owned();
        config.username = "a@b.c".to_owned();
        config.password = Some("pass".to_owned());
        let backend = WebdavBackend::new(&config).unwrap();
        assert_eq!(backend.base_url, "https://dav.jianguoyun.com/dav/manbo");
        assert_eq!(backend.scheme_host, "https://dav.jianguoyun.com");
        assert_eq!(backend.file_url("user.tsv"), "https://dav.jianguoyun.com/dav/manbo/user.tsv");
        assert_eq!(backend.username.as_deref(), Some("a@b.c"));
        assert_eq!(backend.password.as_deref(), Some("pass"));
    }

    #[test]
    fn mkcol_tolerates_existing_but_not_auth_failure() {
        assert!(mkcol_is_fine(StatusCode::CREATED));
        assert!(mkcol_is_fine(StatusCode::METHOD_NOT_ALLOWED));
        // 坚果云对自家 `/dav` 回 403：算「已有」，否则每一轮同步都在第一段放弃、远端永远空着
        assert!(mkcol_is_fine(StatusCode::FORBIDDEN));
        // 凭据不对与服务端错误仍然要报出来，别被这里掩盖
        assert!(!mkcol_is_fine(StatusCode::UNAUTHORIZED));
        assert!(!mkcol_is_fine(StatusCode::INTERNAL_SERVER_ERROR));
    }

    #[test]
    fn tag_falls_back_to_length_and_last_modified() {
        let mut headers = HeaderMap::new();
        headers.insert(header::LAST_MODIFIED, HeaderValue::from_static("Mon, 14 Sep 2026 00:00:00 GMT"));
        assert_eq!(tag_from_headers(&headers, 123), "123-Mon, 14 Sep 2026 00:00:00 GMT");
        headers.insert(header::ETAG, HeaderValue::from_static("\"abc\""));
        assert_eq!(tag_from_headers(&headers, 123), "\"abc\"");
    }
}
