//! 輸出結構與原子寫檔（design.md D6、D10；behavior-inventory B-OUT、B-FLOW-13、B-FLOW-17）。
//!
//! 欄位宣告順序＝B-OUT-6 的鍵序（`serde` 依宣告順序序列化），子結構（B-OUT-7～10）也用 struct。
//! 可省略的鍵（`holidays`、`twii_intraday`、`twii_daily`、`margin`）用 `Option` ＋
//! `skip_serializing_if`：`None` ＝整鍵不出現（B-OUT 表「何時存在」欄）。三個動態桌布鍵的內部
//! 形狀先以 `serde_json::Value` 保存（3.5 再細化），但省略規則是對的。
//!
//! 所有結構同時 `Deserialize`：測試用它確認「已提交的 `expected.json` 讀進來再寫出去不變」
//! （鍵齊全、無多餘欄位、省略規則正確），來源模組也可用 [`PreviousOutput::get`] 取型別化的舊值。
//!
//! # 寫檔
//!
//! [`write_atomic`]：先寫 `tw_events.json.<pid>.tmp`（暫存檔名含行程 ID，與常駐宿主同時寫同一
//! 目錄時不互相破壞，D10）、`sync_all` 後 `std::fs::rename` 取代。Windows 上 Rust 標準庫的
//! `rename` 會取代已存在的目標（測試確認），所以不需要 `MoveFileExW`；標準庫開檔帶
//! `FILE_SHARE_DELETE`，宿主 `data.rs` 輪詢讀檔時也不會擋住取代。UTF-8、無 BOM（B-OUT-2）。

use std::fs;
use std::io;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// 輸出檔名（宿主 `data.rs` 的 `JsonFileSource` 讀同一個檔名）。
pub const OUTPUT_FILE: &str = "tw_events.json";

/// `window`：台北今日起的 `[start, end]` ISO 日期。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub start: String,
    pub end: String,
}

/// `counts`：固定 7 個鍵，順序同 Python（四種事件、再 macro／quotes／punish）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub dividend: usize,
    pub meeting: usize,
    pub conference: usize,
    pub earnings: usize,
    #[serde(rename = "macro")]
    pub macro_events: usize,
    pub quotes: usize,
    pub punish: usize,
}

/// `sources`：純說明字串 11 筆（文字照 Python `update_tw_events.py:1251-1261`），無消費端讀取。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sources {
    #[serde(rename = "macro")]
    pub macro_events: String,
    pub dividend: String,
    pub meeting: String,
    pub conference: String,
    pub earnings: String,
    pub punish: String,
    pub quotes: String,
    pub holidays: String,
    pub twii_intraday: String,
    pub twii_daily: String,
    pub margin: String,
}

impl Default for Sources {
    fn default() -> Self {
        Sources {
            macro_events: "ForexFactory 週曆(本週+下週)".into(),
            dividend: "TWSE 除權除息預告表(TWT48U)".into(),
            meeting: "TWSE OpenAPI t187ap41_L".into(),
            conference: "MOPS 法人說明會一覽表 t100sb02_1（備援：TWSE OpenAPI t187ap04_L 第12款）"
                .into(),
            earnings: "同 conference，市值前百大且擇要訊息驗得出季別者".into(),
            punish: "TWSE OpenAPI announcement/punish + TPEx OpenAPI tpex_disposal_information"
                .into(),
            quotes: "Yahoo Finance chart API + NY Fed SOFR".into(),
            holidays: "TWSE OpenAPI holidaySchedule".into(),
            twii_intraday:
                "Yahoo Finance chart API ^TWII interval=5m&range=5d（最後一個完整交易日）".into(),
            twii_daily: "Yahoo Finance chart API ^TWII interval=1d&range=3mo（已收盤交易日）"
                .into(),
            margin: "TWSE rwd MI_MARGN（MS 彙總＋ALL 個股）＋ OpenAPI STOCK_DAY_ALL 收盤價".into(),
        }
    }
}

/// `macro_meta`：常值（中點是 U+30FB）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroMeta {
    pub countries: String,
    pub importance: String,
}

impl Default for MacroMeta {
    fn default() -> Self {
        MacroMeta {
            countries: "美國\u{30fb}歐元區\u{30fb}日本".into(),
            importance: "中高重要性".into(),
        }
    }
}

/// B-OUT-7：`macro[]` 元素。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroEvent {
    pub dt: String,
    pub date: String,
    pub time: String,
    /// epoch 秒。舊版 Python 產出的檔沒有這個欄位（前端據此退回台北字串，B-DATE-7），沿用舊資料時要
    /// 原樣保留那些列，所以是 `Option`；缺就省略（輸出不出現 `ts`）。本輪新抓的列一律有值。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<i64>,
    pub country: String,
    pub flag: String,
    pub impact: String,
    pub title: String,
    pub title_en: String,
    pub forecast: String,
    pub previous: String,
}

/// 事件類型。**宣告順序＝字母序**（`conference < dividend < earnings < meeting`），因為輸出
/// 依 `(date, type, code)` 排序且 Python 以類型字串比大小；衍生的 `Ord` 因此與 Python 一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventType {
    Conference,
    Dividend,
    Earnings,
    Meeting,
}

/// B-OUT-8：`events[]` 元素，鍵就這 5 個。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub date: String,
    #[serde(rename = "type")]
    pub kind: EventType,
    pub code: String,
    pub name: String,
    pub note: String,
}

/// B-OUT-9：`punish[]` 元素。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Punish {
    pub code: String,
    pub name: String,
    pub start: String,
    pub end: String,
    pub times: i64,
    pub market: String,
}

/// B-OUT-10：`quotes[].kind`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuoteKind {
    Fx,
    Yield,
    Cmdty,
    Index,
    Stock,
}

/// B-OUT-10：`quotes[]` 元素。`prev`、`chg_*` 不可四捨五入（B-QT-9）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quote {
    pub name: String,
    pub kind: QuoteKind,
    pub price: f64,
    pub prev: f64,
    pub chg_abs: f64,
    pub chg_pct: f64,
    /// epoch 秒；Yahoo 無 `regularMarketTime` 時為 `null`（永遠輸出，不省略）。
    pub t: Option<i64>,
}

/// 整份輸出（B-OUT-6 鍵序）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Output {
    /// 本機時間 `%Y-%m-%d %H:%M`（B-DATE-3），每輪必刷新。
    pub updated: String,
    pub fetched: String,
    pub window: Window,
    pub counts: Counts,
    pub errors: Vec<String>,
    /// 動態桌布三鍵的失敗訊息，不進 `errors`；永遠存在。
    pub wallpaper_errors: Vec<String>,
    pub sources: Sources,
    pub macro_meta: MacroMeta,
    #[serde(rename = "macro")]
    pub macro_events: Vec<MacroEvent>,
    pub events: Vec<Event>,
    pub punish: Vec<Punish>,
    pub quotes: Vec<Quote>,
    pub top100: Vec<String>,
    /// 永遠輸出；沒有成功算過就是 `null`。
    pub top100_date: Option<String>,
    /// 本輪抓到、或舊檔有才有；否則整鍵省略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holidays: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub twii_intraday: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub twii_daily: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin: Option<Value>,
}

/// 原子寫入 `<dir>/tw_events.json`：目錄不存在就建；失敗時清掉暫存檔再回報錯誤。
pub fn write_atomic(dir: &Path, output: &Output) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(output).map_err(io::Error::other)?;
    write_bytes_atomic(dir, &bytes)
}

fn write_bytes_atomic(dir: &Path, bytes: &[u8]) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let target = dir.join(OUTPUT_FILE);
    let tmp = dir.join(format!("{OUTPUT_FILE}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut f = fs::File::create(&tmp)?;
        io::Write::write_all(&mut f, bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, &target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// 上一份輸出（B-FLOW-3）：不存在、壞掉、或根不是物件＝空。各鍵型別不符視為不存在（D8-3），
/// 所以來源模組一律經 [`PreviousOutput::get`]（純量、物件鍵）或 [`PreviousOutput::get_items`]
/// （`macro`／`events`／`punish`／`quotes`／`holidays` 這類陣列鍵，**逐筆**判形狀）取值，型別不對就當
/// 沒有舊值（Python 只對動態桌布三鍵這樣做，其餘鍵會崩潰或原樣輸出——刻意改良，見 D8-3；
/// 元素層級的裁定見 task 2 審查 m5：壞元素丟棄、其餘保留）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PreviousOutput {
    map: Map<String, Value>,
}

impl PreviousOutput {
    /// 從已解析的 JSON 建立；根不是物件＝空。
    pub fn from_value(v: Value) -> Self {
        match v {
            Value::Object(map) => PreviousOutput { map },
            _ => PreviousOutput::default(),
        }
    }

    /// 取某鍵並轉成 `T`；鍵不存在、值為 `null`、或型別不符都回 `None`。
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        match self.map.get(key)? {
            Value::Null => None,
            v => T::deserialize(v).ok(),
        }
    }

    /// 取陣列鍵並**逐筆**轉成 `T`：鍵不是陣列（不存在、`null`、純量、物件）回 `None`；是陣列時，
    /// 形狀不符的元素丟棄、其餘依原順序保留（全部壞掉＝`Some(空)`）。Python 沿用舊資料是逐筆原樣
    /// 複製，一個壞元素不該讓整類沿用資料消失（例如舊版 Python 產出、缺 `ts` 的 `macro` 列）。
    pub fn get_items<T: DeserializeOwned>(&self, key: &str) -> Option<Vec<T>> {
        match self.map.get(key)? {
            Value::Array(items) => Some(
                items
                    .iter()
                    .filter_map(|v| T::deserialize(v).ok())
                    .collect(),
            ),
            _ => None,
        }
    }

    /// 原始值（鍵存在即回，含 `null`）。
    pub fn raw(&self, key: &str) -> Option<&Value> {
        self.map.get(key)
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// 讀 `<dir>/tw_events.json` 當上一份輸出；不存在或解析失敗（含 BOM、截斷）＝空。
pub fn read_previous(dir: &Path) -> PreviousOutput {
    fs::read(dir.join(OUTPUT_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .map(PreviousOutput::from_value)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::test_util::TempDir;
    use serde_json::json;

    fn sample() -> Output {
        Output {
            updated: "2026-10-05 01:12".into(),
            fetched: "2026-10-05 01:12".into(),
            window: Window {
                start: "2026-10-05".into(),
                end: "2026-10-19".into(),
            },
            counts: Counts {
                dividend: 1,
                meeting: 0,
                conference: 0,
                earnings: 1,
                macro_events: 1,
                quotes: 1,
                punish: 1,
            },
            errors: vec![],
            wallpaper_errors: vec!["加權日K來源失敗：x".into()],
            sources: Sources::default(),
            macro_meta: MacroMeta::default(),
            macro_events: vec![MacroEvent {
                dt: "2026-09-28T21:30".into(),
                date: "2026-09-28".into(),
                time: "21:30".into(),
                ts: Some(1790602200),
                country: "EU".into(),
                flag: "歐".into(),
                impact: "medium".into(),
                title: "歐洲央行總裁拉加德談話".into(),
                title_en: "ECB President Lagarde Speaks".into(),
                forecast: "".into(),
                previous: "".into(),
            }],
            events: vec![Event {
                date: "2026-10-05".into(),
                kind: EventType::Earnings,
                code: "2308".into(),
                name: "台達電".into(),
                note: "Q2 財報".into(),
            }],
            punish: vec![Punish {
                code: "2030".into(),
                name: "彰源".into(),
                start: "2026-10-02".into(),
                end: "2026-10-08".into(),
                times: 1,
                market: "上市".into(),
            }],
            quotes: vec![Quote {
                name: "USD/TWD".into(),
                kind: QuoteKind::Fx,
                price: 31.5,
                prev: 31.867900848388672,
                chg_abs: -0.3679,
                chg_pct: -1.15,
                t: None,
            }],
            top100: vec!["2330".into()],
            top100_date: None,
            holidays: None,
            twii_intraday: None,
            twii_daily: None,
            margin: None,
        }
    }

    fn keys(v: &Value) -> Vec<&str> {
        v.as_object().unwrap().keys().map(String::as_str).collect()
    }

    /// `serde_json::Value` 預設以字母序存鍵，看不出序列化順序，所以直接看輸出文字裡鍵的位置。
    fn top_level_key_order(text: &str) -> Vec<String> {
        // pretty 輸出的頂層鍵恰好縮排 2 格：`  "key": ...`。
        text.lines()
            .filter(|l| l.starts_with("  \"") && !l.starts_with("   "))
            .filter_map(|l| l.trim_start().split('"').nth(1).map(str::to_string))
            .collect()
    }

    #[test]
    fn key_order_is_b_out_6() {
        let mut out = sample();
        out.holidays = Some(vec!["2026-10-10".into()]);
        out.twii_intraday = Some(json!({"date": "2026-10-02", "points": []}));
        out.twii_daily = Some(json!([]));
        out.margin = Some(json!({}));
        let text = serde_json::to_string_pretty(&out).unwrap();
        assert_eq!(
            top_level_key_order(&text),
            [
                "updated",
                "fetched",
                "window",
                "counts",
                "errors",
                "wallpaper_errors",
                "sources",
                "macro_meta",
                "macro",
                "events",
                "punish",
                "quotes",
                "top100",
                "top100_date",
                "holidays",
                "twii_intraday",
                "twii_daily",
                "margin",
            ]
        );
    }

    #[test]
    fn omitted_keys_are_absent_but_null_keys_stay() {
        let v = serde_json::to_value(sample()).unwrap();
        for k in ["holidays", "twii_intraday", "twii_daily", "margin"] {
            assert!(v.get(k).is_none(), "{k} 應省略");
        }
        // `top100_date`、`quotes[].t` 是 null、不是省略；`wallpaper_errors` 永遠存在。
        assert_eq!(v["top100_date"], Value::Null);
        assert_eq!(v["quotes"][0]["t"], Value::Null);
        assert!(v.get("wallpaper_errors").is_some());
        assert_eq!(
            keys(&v["counts"]).len(),
            7,
            "counts 固定 7 個鍵：{:?}",
            keys(&v["counts"])
        );
        assert_eq!(keys(&v["sources"]).len(), 11);
        assert_eq!(v["macro_meta"]["countries"], "美國・歐元區・日本");
        // events 元素鍵固定 5 個，型別欄位名是 `type`。
        assert_eq!(
            keys(&v["events"][0]),
            ["code", "date", "name", "note", "type"]
        );
        assert_eq!(v["events"][0]["type"], "earnings");
        assert_eq!(v["quotes"][0]["kind"], "fx");
    }

    #[test]
    fn event_type_order_matches_python_string_order() {
        let mut v = [
            EventType::Meeting,
            EventType::Earnings,
            EventType::Dividend,
            EventType::Conference,
        ];
        v.sort();
        let names: Vec<String> = v
            .iter()
            .map(|t| {
                serde_json::to_value(t)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        let mut by_string = names.clone();
        by_string.sort();
        assert_eq!(names, by_string);
    }

    #[test]
    fn write_atomic_writes_valid_utf8_without_bom_and_leaves_no_tmp() {
        let dir = TempDir::new("output-write");
        let out = sample();
        write_atomic(dir.path(), &out).unwrap();
        let bytes = fs::read(dir.path().join(OUTPUT_FILE)).unwrap();
        assert!(!bytes.starts_with(&[0xEF, 0xBB, 0xBF]), "不可有 BOM");
        let text = String::from_utf8(bytes.clone()).expect("UTF-8");
        assert!(text.contains("歐洲央行總裁拉加德談話"), "非 ASCII 原樣輸出");
        let parsed: Output = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed, out);
        // 暫存檔不殘留：目錄裡只有 tw_events.json。
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [OUTPUT_FILE]);
    }

    #[test]
    fn write_atomic_replaces_existing_target() {
        let dir = TempDir::new("output-replace");
        fs::write(
            dir.path().join(OUTPUT_FILE),
            b"OLD CONTENT, much longer than the new file would not be needed",
        )
        .unwrap();
        let mut out = sample();
        write_atomic(dir.path(), &out).unwrap();
        out.updated = "2026-10-06 00:00".into();
        write_atomic(dir.path(), &out).unwrap();
        let parsed: Output =
            serde_json::from_slice(&fs::read(dir.path().join(OUTPUT_FILE)).unwrap()).unwrap();
        assert_eq!(parsed.updated, "2026-10-06 00:00");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn write_atomic_creates_missing_directories() {
        let dir = TempDir::new("output-mkdir");
        let nested = dir.path().join("a").join("b");
        write_atomic(&nested, &sample()).unwrap();
        assert!(nested.join(OUTPUT_FILE).is_file());
    }

    #[test]
    fn write_atomic_replaces_target_even_while_a_reader_has_it_open() {
        // 宿主 `data.rs` 輪詢讀檔時，不可因為讀者開著檔案就取代失敗（標準庫開檔帶 FILE_SHARE_DELETE）。
        let dir = TempDir::new("output-open-reader");
        write_atomic(dir.path(), &sample()).unwrap();
        let reader = fs::File::open(dir.path().join(OUTPUT_FILE)).unwrap();
        let mut out = sample();
        out.updated = "2026-10-07 07:07".into();
        write_atomic(dir.path(), &out).unwrap();
        drop(reader);
        assert_eq!(
            read_previous(dir.path())
                .get::<String>("updated")
                .as_deref(),
            Some("2026-10-07 07:07")
        );
    }

    #[test]
    fn failed_write_cleans_up_the_tmp_file() {
        let dir = TempDir::new("output-fail");
        // 讓目標變成「目錄」：rename 檔案到目錄上必失敗，暫存檔要被清掉。
        fs::create_dir(dir.path().join(OUTPUT_FILE)).unwrap();
        assert!(write_atomic(dir.path(), &sample()).is_err());
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "殘留暫存檔：{leftovers:?}");
    }

    #[test]
    fn read_previous_missing_corrupt_bom_and_non_object_are_empty() {
        let dir = TempDir::new("output-prev-empty");
        assert!(read_previous(dir.path()).is_empty()); // 不存在
        let f = dir.path().join(OUTPUT_FILE);
        fs::write(&f, b"{ not json").unwrap();
        assert!(read_previous(dir.path()).is_empty()); // 壞掉
        fs::write(&f, b"[1,2,3]").unwrap();
        assert!(read_previous(dir.path()).is_empty()); // 根不是物件
        fs::write(&f, b"\xEF\xBB\xBF{\"a\":1}").unwrap();
        assert!(read_previous(dir.path()).is_empty()); // 有 BOM（同 Python json.loads 失敗）
        fs::write(&f, br#"{"a":1}"#).unwrap();
        assert!(!read_previous(dir.path()).is_empty());
    }

    #[test]
    fn previous_get_returns_none_for_missing_null_and_wrong_type() {
        let prev = PreviousOutput::from_value(json!({
            "top100": ["2330", "2317"],
            "top100_date": "2026-10-04",
            "quotes": "oops",          // 型別錯誤
            "macro": {"not": "list"},  // 型別錯誤
            "holidays": null,
            "events": [{"date": "x"}], // 元素缺欄位
            "twii_daily": [{"date": "2026-10-02", "open": 1.0}],
        }));
        assert_eq!(
            prev.get::<Vec<String>>("top100"),
            Some(vec!["2330".into(), "2317".into()])
        );
        assert_eq!(
            prev.get::<String>("top100_date").as_deref(),
            Some("2026-10-04")
        );
        assert_eq!(prev.get::<Vec<Quote>>("quotes"), None);
        assert_eq!(prev.get::<Vec<MacroEvent>>("macro"), None);
        assert_eq!(prev.get::<Vec<String>>("holidays"), None);
        assert_eq!(prev.get::<Vec<Event>>("events"), None);
        assert_eq!(prev.get::<Vec<String>>("absent"), None);
        // top100 當整數／陣列當字串，同樣當作不存在。
        assert_eq!(prev.get::<i64>("top100"), None);
        assert_eq!(prev.get::<String>("top100"), None);
        // 原始值仍可取（含 null）。
        assert_eq!(prev.raw("holidays"), Some(&Value::Null));
        assert!(prev.get::<Value>("twii_daily").is_some());
    }

    #[test]
    fn macro_event_without_ts_round_trips_without_inventing_one() {
        // 舊版 Python 產出的 macro 列沒有 ts（前端退回台北字串，B-DATE-7）：沿用時要保留該列、
        // 重新輸出時仍不帶 ts。
        let old = json!({
            "dt": "2026-09-28T21:30", "date": "2026-09-28", "time": "21:30",
            "country": "EU", "flag": "歐", "impact": "medium", "title": "a",
            "title_en": "a", "forecast": "", "previous": ""
        });
        let ev: MacroEvent = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(ev.ts, None);
        assert_eq!(serde_json::to_value(&ev).unwrap(), old);
    }

    #[test]
    fn previous_get_items_drops_bad_elements_but_keeps_the_rest() {
        let good = json!({
            "dt": "2026-09-28T21:30", "date": "2026-09-28", "time": "21:30", "ts": 1,
            "country": "EU", "flag": "歐", "impact": "medium", "title": "a",
            "title_en": "a", "forecast": "", "previous": ""
        });
        let no_title = {
            let mut v = good.clone();
            v.as_object_mut().unwrap().remove("title");
            v
        };
        let prev = PreviousOutput::from_value(json!({
            "macro": [good.clone(), no_title, "oops", 5, null, good.clone()],
            "events": {"x": 1},
            "quotes": "bad",
            "holidays": ["2026-10-10", 7, null, "2026-10-11"],
            "punish": null,
            "all_bad": [1, 2],
        }));
        let got: Vec<MacroEvent> = prev.get_items("macro").unwrap();
        assert_eq!(got.len(), 2, "缺必要欄位與非物件的元素丟棄、好的保留");
        assert_eq!(got[0].title, "a");
        // 鍵不是陣列＝整鍵不存在（不存在、物件、字串、null）。
        assert_eq!(prev.get_items::<Event>("events"), None);
        assert_eq!(prev.get_items::<Quote>("quotes"), None);
        assert_eq!(prev.get_items::<Punish>("punish"), None);
        assert_eq!(prev.get_items::<Event>("absent"), None);
        // 純量元素同樣逐筆判斷；全壞＝空陣列（鍵存在、是陣列）。
        assert_eq!(
            prev.get_items::<String>("holidays"),
            Some(vec!["2026-10-10".to_string(), "2026-10-11".to_string()])
        );
        assert_eq!(prev.get_items::<String>("all_bad"), Some(vec![]));
    }

    #[test]
    fn written_file_is_readable_as_previous() {
        let dir = TempDir::new("output-roundtrip");
        let mut out = sample();
        out.holidays = Some(vec!["2026-10-10".into()]);
        out.top100_date = Some("2026-10-05".into());
        write_atomic(dir.path(), &out).unwrap();
        let prev = read_previous(dir.path());
        assert_eq!(
            prev.get::<Vec<String>>("holidays"),
            Some(vec!["2026-10-10".into()])
        );
        assert_eq!(
            prev.get::<String>("top100_date").as_deref(),
            Some("2026-10-05")
        );
        assert_eq!(prev.get::<Vec<Quote>>("quotes"), Some(out.quotes.clone()));
        assert_eq!(prev.get::<Vec<Event>>("events"), Some(out.events.clone()));
    }
}
