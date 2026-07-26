# SPEC：週單位改週日起算 ＋ 前百大財報公布日

日期：2026-07-26　狀態：設計定案，待實作

## 目標

1. 台股固定事件的「本週」由**週一–週日**改為**週日–週六**。
2. 台股動態事件上節加入**市值前百大公司的財報公布日**，與法說會・股東會同欄。

---

## API 驗證結論（2026-07-26 實測）

### 沒有官方「財報公布日」預告來源

| 來源 | 結果 |
| --- | --- |
| TWSE openapi（143 端點全掃） | 財報相關只有 `t187ap06/07`＝已公布的**報表內容**，無公布日程端點 |
| Yahoo `quoteSummary?modules=calendarEvents` | **HTTP 401**（query1／query2 皆是），需 crumb+cookie |
| TWSE `t187ap04_L` 每日重大訊息 | 僅**當日** 8 筆，主旨含「財務報告」**0 筆** |

### 採用來源：MOPS 法人說明會一覽表

`POST https://mopsov.twse.com.tw/mops/web/ajax_t100sb02_1`
（form：`encodeURIComponent=1&step=1&firstin=1&off=1&TYPEK=sii&year=<民國年>&month=<MM>`）

- 回傳 HTML 表格。**必須走 `CERT_NONE` 退回**（Py3.13 對 twse 憑證鏈更嚴，同 CLAUDE.md 既載的坑）。
- 實測場次：`sii` 115/07 → 82 場、115/08 → 23 場（**含未來場次**）。
- 欄位順序：公司代號｜公司名稱｜召開日期｜時間｜地點｜擇要訊息｜簡報內容｜…
- 擇要訊息即財報聲明，如台積電 115/07/16「公布本公司 2026 年第 2 季財務報告及 2026 年第 3 季業績展望」。
- **日期欄有兩種格式**：`115/07/16`，以及區間 `115/06/30 至 115/07/03`（105 筆中 8 筆）→ 一律**取起日**。
- `otc` 不抓：實測前百大**無一家**上櫃。

### 市值前百大：自算

`t187ap03_L`（1092 家，`已發行普通股數或TDR原股發行股數` 欄 1092 家全有值）
× `STOCK_DAY_ALL`（`ClosingPrice`）→ 算得 1089 家市值。

抽驗：台積電 60.9 兆居首、聯發科 6.0 兆、台達電 4.6 兆；第 100 名勤誠 1372 億。排序合理。

成本：`t187ap03_L` 1.3 MB ＋ `STOCK_DAY_ALL` 311 KB＝每次約 1.6 MB → **每日只算一次**（快取於輸出檔）。

---

## 設計

### ① 週單位（`finance-calendar.html`）

單行修改，第 301 行：

```js
const weekStartOf = d => addDays(strip(d), -((d.getDay() + 6) % 7)); // 週一為首
→
const weekStartOf = d => addDays(strip(d), -d.getDay());             // 週日為首
```

全檔僅 `renderFixed()`（第 542 行）使用；`we = addDays(ws, 6)` 自動變成週六。
既有 `dailyReloadAt: '04:00'` 讓窗口於週日凌晨翻週，無須額外處理。

### ② 資料層（`update_tw_events.py`）

**新常數**

```python
URL_MOPS_CONF = "https://mopsov.twse.com.tw/mops/web/ajax_t100sb02_1"
URL_BASIC     = "https://openapi.twse.com.tw/v1/opendata/t187ap03_L"
URL_DAY_ALL   = "https://openapi.twse.com.tw/v1/exchangeReport/STOCK_DAY_ALL"
TOP_N = 100
```

**`http_bytes(url, data=None)`** — 從現有 `http_json` 抽出「取 bytes ＋ SSL 退回」共用邏輯；
`http_json` 改為 `json.loads(http_bytes(url).decode("utf-8"))`。
`except` 內的判斷式**原封不動搬移**（同時接 `ssl.SSLError` 與 `URLError`，僅 SSL 問題才退回不驗證，HTTPError 照原樣往上丟）。
`data` 非 None 時以 POST form 送出。

**`mops_rows(text) -> list[list[str]]`** — regex 解析 `<tr>`／`<td>`、去標籤、`html.unescape`。
只收第一欄符合 `^\d{4}$` 的資料列（自動濾掉兩列表頭）。

**`fetch_top100(errors, old) -> list[str] | None`**

- `old["top100_date"] == 今天（台北）` 且 `old["top100"]` 非空 → 直接回傳舊名單，**不發任何請求**。
- 否則抓兩個端點，市值 ＝ 已發行普通股數 × 收盤價，取前 100 的公司代號。
- 例外 → `errors.append(...)` 並回 `None`（由 `main` 沿用 `old["top100"]`）。

**`fetch_conference(errors, top100) -> list | None`**（改寫）

- 抓 MOPS 一覽表，`TYPEK=sii`，**本月＋下月**（民國年月，跨年時年份進位）。
- 只保留 `code in top100` 的場次（非前百大不收）。
- 季別偵測（依序）：`第\s*([一二三四1-4])\s*季` → `([1-4])Q\d{2}` → `Q([1-4])`。
- 分類：驗出季別 → `type="earnings"`、`note="Q2 財報"`；
  否則 → `type="conference"`、`note="法說會"`。
- 地點或擇要訊息含「線上」→ note 追加「（線上）」（沿用現行行為）。
- 失敗 → 退回舊來源 `fetch_conference_news()`（現有 `t187ap04_L` 第 12 款版，改名保留）；
  兩者皆失敗才回 `None`。
- **`top100` 為空時直接視為來源失敗**（回 `None`，走上面那條退回鏈）。
  因為「只收前百大」在名單空的情況下等於一場都不收，靜默產出空清單會被誤讀成「近兩週沒有財報」。

**`main()`**

- `fetch_top100` 在 `fetch_conference` 之前呼叫，結果傳入。
- `counts` 的類型元組加 `"earnings"`。
- 輸出檔新增 `top100`（代號陣列）與 `top100_date`（`YYYY-MM-DD`，台北）。
- 失敗沿用：`_old_events_of` 需同時取 `conference` 與 `earnings` 兩型。
- 事件去重鍵 `(type, code, date)` 不變——單一來源不會讓同 code+date 同時產出兩種型別。

### ③ 前端（`finance-calendar.html`）

- 新增 CSS 變數 `--c-earn`，色相與既有 `--c-div`／`--c-meet`／`--c-conf`／`--c-punish` 分開。
- `TYPE_META.earnings = { chip: '財報', cls: 'var(--c-earn)' }`。
- `CONFIG.dynTypes` → `['earnings', 'conference', 'meeting', 'punish']`。
  earnings 排最前，使同日財報排在法說會之前（排序第二鍵是 `dynTypes` 索引）。
  檔頭 `CONFIG` 說明區（第 34 行）的 `dynTypes` 預設值文字要同步更新。
- 上節標題由既有 `upTypes.map(t => TYPE_META[t].chip).join('・')` 自動變成
  「財報・法說會・股東會｜近兩週」——**無須修改該行**。
- `const VERSION` 遞增。

---

## 韌性

| 失敗點 | 行為 |
| --- | --- |
| MOPS 一覽表 | 退回舊 `t187ap04_L` 第 12 款來源 |
| 兩個法說會來源皆失敗 | 沿用上次輸出的 `conference` ＋ `earnings` 事件 |
| top100 抓取失敗 | 沿用 `old["top100"]` |
| 連舊 top100 都沒有（首次執行即斷網） | `fetch_conference` 視為來源失敗 → 退回舊 `t187ap04_L` 來源 → 再不行才沿用上次 events |

## 預期結果（以 2026-07-26 起 14 天窗口實測）

窗口內上市法說會 38 場，其中前百大 19 場 → 只收這 19 場：
18 場判為「財報」，1 場（7769 鴻勁「受邀參加凱基證券舉辦之線上法人說明會」，無季別）判為「法說會」。

## 驗證方式

- `python update_tw_events.py` — 檢查各來源筆數、`earnings` 計數、鏡像目標命中。
- 針對 `mops_rows` 的區間日期（`115/06/30 至 115/07/03`）與季別偵測寫最小測試。
- headless Edge 截圖：`msedge --headless=new --disable-gpu --window-size=W,H --virtual-time-budget=6000 --screenshot=<路徑> "file:///<絕對路徑>"`。
- `window.__TEST_TODAY='YYYY-MM-DD'` 覆寫今天，驗證週日–週六窗口與翻週。
