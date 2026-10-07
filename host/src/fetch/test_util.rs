//! 測試共用小工具（僅 `cfg(test)`）。

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// 在 current-thread runtime 上跑完一個 future（`tokio` 只開 `time`／`rt`，不用 `#[tokio::test]`）。
pub fn block_on<F: Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("建立測試用 tokio runtime")
        .block_on(f)
}

/// 唯一的暫存資料夾（行程 ID＋計數器），Drop 時刪除；不新增 `tempfile` 依賴。
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "fc-host-fetch-test-{}-{}-{tag}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("建立暫存資料夾");
        TempDir(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ───────────────────────── 測試用 Fetch ─────────────────────────

use std::collections::HashMap;
use std::sync::Mutex;

use super::errors::FetchError;
use super::http::{Fetch, Request, Response};

/// 以網址（忽略方法與表單）對應固定回應的 `Fetch`：測試要手工造一小段 JSON、不想為它開一個
/// 樣本情境時用。沒登記的網址回 `FetchError::Network`；每個請求都記錄下來。
#[derive(Default)]
pub struct MapFetch {
    routes: HashMap<String, Result<Response, FetchError>>,
    requested: Mutex<Vec<String>>,
}

impl MapFetch {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登記一個 200 的 JSON／文字本體。
    pub fn ok(mut self, url: &str, body: &str) -> Self {
        self.routes.insert(
            url.to_string(),
            Ok(Response {
                status: 200,
                body: body.as_bytes().to_vec(),
            }),
        );
        self
    }

    /// 登記任意狀態碼（如 404／500）。
    pub fn status(mut self, url: &str, status: u16) -> Self {
        self.routes.insert(
            url.to_string(),
            Ok(Response {
                status,
                body: Vec::new(),
            }),
        );
        self
    }

    /// 登記連線層失敗。
    pub fn network_error(mut self, url: &str, msg: &str) -> Self {
        self.routes
            .insert(url.to_string(), Err(FetchError::Network(msg.to_string())));
        self
    }

    /// 依發出順序的請求網址。
    pub fn requests(&self) -> Vec<String> {
        self.requested
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl Fetch for MapFetch {
    async fn fetch(&self, req: &Request) -> Result<Response, FetchError> {
        self.requested
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(req.url.clone());
        match self.routes.get(&req.url) {
            Some(r) => r.clone(),
            None => Err(FetchError::Network(format!("MapFetch 未登記：{}", req.url))),
        }
    }
}

/// 任何請求都 panic 的 `Fetch`：測試「來源裡 panic 時改沿用舊資料」（D7）。
pub struct PanicFetch;

impl Fetch for PanicFetch {
    async fn fetch(&self, req: &Request) -> Result<Response, FetchError> {
        panic!("注入的 panic（{}）", req.url)
    }
}

/// 讀 `tests/fixtures/`（repo 根目錄、Python 單元測試的真實錄製回應，**唯讀**）下的 JSON。
pub fn py_fixture(name: &str) -> serde_json::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("fixtures")
        .join(name);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("讀不到 {}：{e}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("解析 {} 失敗：{e}", path.display()))
}
