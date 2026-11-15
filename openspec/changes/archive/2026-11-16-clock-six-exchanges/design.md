## Context

動機見 proposal.md。現況：

- `host/ui/widgets/clock.js` 每秒重畫三行文字，不吃任何資料。
- 六個交易所的時段與休市表在 `host/ui/wallpapers/config/wallpaper-config.default.json`（`markets`、`holidays`），
  判斷函式在 `host/ui/wallpapers/lib/astrolabe-sessions.mjs` 的 `marketState`（處理時區、休市日、半日市、
  `effective`、`skip`）。兩者都在 `host/ui/` 之下，小工具頁面可以直接 import／fetch。
- 時鐘的最小框與舒適框同為 212×160（`widgets.rs` 的 `WIDGET_SPECS`），倍率公式讓內容填滿框；
  `layout.rs` 與前端有多支測試綁著這組數字。

## Goals / Non-Goals

**Goals:**

- 開收盤判斷與動態桌布同源，不另寫一套。
- 新版面在現有的 212×160 框內放得下，Rust 端零改動。

**Non-Goals:**

- 不讀使用者的 `wallpaper-config.json`（要讀得新增資料通道）。
- 不自動抓取各交易所日曆；休市表仍是手動維護。
- 不顯示秒、不顯示本機時間、不顯示午休或盤前盤後的細分狀態。

## Decisions

### D1：判斷邏輯放純函式模組 `host/ui/exchange-clock.mjs`

`exchangeRows(config, nowMs)` 回傳六筆 `{ code, time, date, open }`，內部呼叫 `marketState` 與 `tzParts`。
`open`＝`marketState(...).status` 為 `regular` 或 `auction`（`STATUS_RANK` 取最高者，所以午休、只有期貨夜盤、
休市日都不會是這兩個值；半日市由 `expandMarket` 截短）。`config` 為 `null` 時仍回傳時間與日期、`open` 一律 `false`。

簡寫對照表放在這個模組：`TYO→JP、SEL→KR、TPE→TW、HKG→HK、LON→UK、NYC→US`，順序也由它決定（由東到西）。
設定檔找不到某個市場 id 時，該列用模組內建的時區照常顯示時間、`open` 為 `false`。

**為什麼不直接寫在 `clock.js`**：`mount()` 要 DOM，純函式才能用 Node 內建測試直接驗各種時刻。
**為什麼不另寫判斷**：會跟桌布各算各的，休市表更新後容易只改到一邊。

### D2：`clock.js` 以 `loadDefaultConfig()` 讀內建表

掛載時先以 `config = null` 畫一次（全灰），`loadDefaultConfig()` 回來後重畫；失敗記 `console.warn`、維持全灰。
每秒 `tick()` 只在文字或開收盤狀態改變時寫 DOM。`visibilitychange` 的補畫保留。

替代方案：把市場表經 Rust 新通道推給時鐘（能反映使用者改過的 `wallpaper-config.json`）。要動 `data.rs`、
`may_access_channel` 與 IPC 守門測試，對這個需求過重，先不做。

### D3：版面為單欄六列，沿用 212×160 的框

每列一個三欄 grid（簡寫、時間、日期），欄寬固定讓六列對齊，數字用 `tabular-nums`。時間是主角（字級最大、粗體），
開盤 `color: #fff`、收盤 `color: var(--dim)`；簡寫與日期固定用次要顏色。確切字級以量測結果為準，條件是整扇
視窗的自然尺寸不超過 212×160（含 `--widget-gap` 與 4 px 字型餘裕）。

替代方案：兩欄三列（每格一行三項）。自然寬度約 300 px，要放大最小框，`layout.rs` 與前端十多支綁 212×160 的
測試都得跟著改，換到的只是寬扁一點的比例；使用者可以用拖曳把時鐘框拉大，倍率會跟著放大。

### D4：對照與量測工具

- 開收盤判斷的正確性由 `host/tests/exchange-clock.test.mjs` 負責；顯示文字由 `compare.mjs` 的基準檔
  `golden/clock.txt` 把關（對照工具在 change `retire-lively` 改為基準檔比對，原本的 `panels.mjs` 已刪）。
- `measure-clock-natural-width.mjs` 改量新版面：六列各取最寬字串（時間枚舉 1440 個、日期枚舉數年），
  輸出自然寬高並與 212×160 比較。

## Risks / Trade-offs

- [內建休市表沒有的日子會被當成交易日]（香港 2027-10-08 以後）→ 休市日的盤中時段會顯示白色。台北 2027 已在本 change
  依 2026-11-14 錄下的 TWSE 回應補進內建表；香港與之後的年度補表另記 kata 待辦。
- [內建時段表只能表達「提早收盤」，表達不了延後開盤]（KRX 修能考當天與年度第一個交易日 10:00 開盤）→ 那兩天
  首爾 09:00–10:00 會顯示白色、延後的收盤時段顯示灰色。屬動態桌布時段模型的既有簡化，另記 kata。
- [使用者改了 `wallpaper-config.json` 的時段或休市日，時鐘不跟] → 已列 Non-Goals；之後要做再開通道。
- [使用者不在台灣時看不到本機時間] → 使用者已確認拿掉本機大時鐘。
- [單欄六列與討論時的兩欄草圖不同] → 理由見 D3，驗收時由使用者確認觀感。
