//! 資料層抓取（data-layer-rust，design.md D6）。骨架（task 2.1＋2.2）：
//!
//! - [`http`]：[`http::Fetch`] 介面、reqwest 實作、同主機請求間隔、與 Python 一致的表單編碼。
//! - [`clock`]：時間注入（台北固定 +08:00／本機牆上時間，D11）。
//! - [`dates`]：民國年轉換、與 Python 逐位一致的 `round2` 與 `fmt_g`（K-4、K-6）。
//! - [`output`]：輸出結構（B-OUT 鍵序）、原子寫檔、讀上一份輸出。
//! - [`errors`]：[`errors::FetchError`] 與錯誤訊息正規化（D4）。
//! - `fixture`（僅測試）：`FixtureFetch`，語意對照 `tests/fetch_oracle/expect.py` 的回放。
//! - [`json_util`]：Python 真值（`x or ""`）與 `str(x)` 語意的小工具。
//! - [`outcome`]：各來源共用的 [`outcome::SourceOutcome`] 與 panic 隔離 [`outcome::guarded`]（D7）。
//! - 來源模組（每個一個檔，只靠 `Fetch`＋`Clock`＋上一份輸出就能實作與測試）：[`macro_ff`]、
//!   [`sofr`]、[`holidays`]（3.1）、[`dividend`]、[`meeting`]、[`top100`]（3.2）、[`conference`]（3.3；另有 [`html_text`]、[`html_entities`]：Python `html.unescape` 與其具名表）、[`punish`]、[`quotes`]（3.4）、[`twii`]（盤中走勢＋日 K）、[`margin`]（券資比；3.5。這三個動態桌布鍵回傳 [`outcome::WallpaperOutcome`]：錯誤進 `wallpaper_errors`、不計 `fresh`）。
//! - [`events`]：台股事件的窗口、去重、排序（B-EVT）與舊事件沿用，3.2～3.3 的來源與 4.1 組裝共用。
//! - `oracle`（僅測試）：對每個樣本情境與 Python 的 `expected.json` 對照的共用小工具。
//! - [`round`]：一輪的組裝（`run_round`）與寫檔（`write_round`），4.1；對照 Python `main()`。
//! - [`net`]：啟動時等網路（B-FLOW-2），5.1；[`report`]：抓取日誌（寫進宿主記錄檔），5.4；
//!   [`cli`]：`--fetch-once <目錄>` 單次抓取指令，5.1。
//! - [`exec`]：執行一輪並寫檔（`--fetch-once` 與排程共用），5.2；[`sched`]：排程的純判斷（台北時點、補抓、60 分鐘
//!   下限、失敗重試），5.2；[`state`]：排程紀錄檔 `fetch-state.json`，5.2；[`scheduler`]：醒來迴圈、專用執行緒、
//!   停止介面與宿主接線，5.2＋5.3；[`status`]：設定視窗的抓取狀態查詢指令，5.5。

pub mod cli;
pub mod clock;
pub mod conference;
pub mod dates;
pub mod dividend;
pub mod errors;
pub mod events;
pub mod exec;
pub mod holidays;
pub mod html_entities;
pub mod html_text;
pub mod http;
pub mod json_util;
pub mod macro_ff;
pub mod margin;
pub mod meeting;
pub mod net;
pub mod outcome;
pub mod output;
pub mod punish;
pub mod quotes;
pub mod report;
pub mod round;
pub mod sched;
pub mod scheduler;
pub mod sofr;
pub mod state;
pub mod status;
pub mod top100;
pub mod twii;

#[cfg(test)]
pub mod fixture;
#[cfg(test)]
pub mod oracle;
#[cfg(test)]
pub mod test_util;
