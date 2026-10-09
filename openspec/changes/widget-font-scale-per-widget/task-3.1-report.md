# Task 3.1 報告：清單收合版面與最小框實測

依 design.md D5 與 spec「清單在窄寬度收合」：清單列在窄寬度改為兩行，並以 headless Edge 實測各清單的單行版面斷點與收合版面最窄可用寬度，`WIDGET_SPECS` 清單 `min_width` 改為實測值。

## 結論

| 小工具 | 設計寬（comfort） | 收合斷點（`width <`） | `min_width`（舊值） | ÷ 設計寬 | D5 目標 ≤ 0.55 |
| --- | --- | --- | --- | --- | --- |
| 總經日曆 | 500 | 423 | 229（375） | 0.458 | 達成（亦 ≤ 266） |
| 台股固定事件 | 470 | 無（沒有右側數值） | 274（352.5） | 0.583 | 未達（差 15.5） |
| 台股動態事件 | 470 | 467 | 333（352.5） | 0.709 | 未達（差 74.5） |
| 擴充插槽 | 470 | 無（沒有清單列） | 239（352.5） | 0.509 | 達成 |

- 總經日曆 `min_width` 229 ≤ 266：spec「窄框的清單可以放大」情境（邏輯寬 320、高度充足、字級 200%）實測倍率 1.28、CSS 寬 250，收合版面 0 個問題。
- 台股固定事件與台股動態事件未達 D5 的 0.55 目標，依 D5「回報而不是硬壓數值」照實測值設定。限制來源見下方「未達目標的原因」。
- 所有 `min_width` 都比舊值小，最小格數只會放寬（Rust 屬性測試 `new_min_size_is_never_stricter_than_old_for_widgets_whose_min_height_did_not_grow` 照常通過）；`comfort` 不變。
- 斷點都小於設計寬。倍率由寬度決定時 CSS 寬剛好等於設計寬（`verify-visual-edges.mjs` 預設格實測 viewport 為 500／470／470），所以斷點不能等於設計寬，否則浮點誤差可能讓預設版面誤切成收合版面。

## 實作

- `host/ui/widget.html`：把 `?w=<id>` 寫到 `<html data-widget>`，讓 CSS 依小工具各自的斷點收合。
- `host/ui/widget.css`「清單收合」一節：
  - 總經日曆 `@media (width < 423px)`、台股動態事件 `@media (width < 467px)`：`.ev` 改成 `flex-wrap: wrap`；標題 `b` 設 `flex: 1 1 0; min-width: 0; overflow-wrap: anywhere`，吃剩餘寬度、可在任何字元處換行；`.n` 設 `flex: 1 0 100%; margin-left: 0`，獨占第二行、沿用原本的靠右；空的 `.n` 設 `display: none`，不多佔一行。
  - D5 寫的是「兩行 grid」，實作改用 flex 換行：第一行（時間、標記、點、標題）、第二行（數值靠右）的結果相同，但不必為各小工具不同的子元素數量各寫一套 grid 欄位。
  - 不限寬度的兩條規則：`.panel > header { flex-wrap: wrap }`，讓標題列放不下時副標題移到第二行，`.ttl` 不被擠成兩行；`.empty`／`.hint`／`.dayhead`／`.divider`／`.summary` 設 `overflow-wrap: anywhere`，讓長英數字串（資料目錄路徑、JSON 鍵名）不撐出面板。內容放得下時沒有作用：設計寬下改動前後版面逐元素相同（見「核對」）。
- `host/src/widgets.rs`：`WIDGET_SPECS` 清單 `min_width` 改為上表數值，文件表格與說明同步更新。

## 量測方法

腳本：`host/tests/compare/measure-list-collapse.mjs`，零安裝（Node 22 內建 CDP），作法比照 `measure-clock-natural-width.mjs`。

1. 以 headless Edge（獨立 `--user-data-dir`）開 `widget.html?w=<id>&fixtures=…`，`deviceScaleFactor` 1、無 ZoomFactor（倍率 1），Date 固定在指定日 23:58:58、`__TEST_TODAY` 同日。
2. 資料換成最差情況 fixture（見下節），伺服器暫存目錄與 profile 用完即刪。
3. 以 `Emulation.setDeviceMetricsOverride` 逐 1 px 改變 viewport 寬，每個寬度在頁面內檢查。

   原始判準（spec）：

   - A：每列 `.ev` 的子元素兩兩不重疊，且都在列的內容框內。
   - B：文字不被裁切。`.ev` 子元素、標題列、`.dayhead`／`.divider`／`.empty`／`.hint`／`.summary` 的文字行盒（Range `getClientRects`）都在自己的內容框內；面板、捲動區、header 的 `scrollWidth ≤ clientWidth`。
   - C：標題列 `.ttl`／`.sub` 不重疊，且都在 header 內。

   可讀性下限（本 task 另加，只會讓結果更保守）：

   - R1：清單標題換行時，寬度不小於 4 個全形字（4em）。標題本身短於 4em、一行放得下時不算。少了這條，原始判準允許標題一字一行。
   - R2：右側數值 `.n` 不換行。
   - R3：小工具標題 `.ttl` 不換行。
4. 掃描模式。「N」＝從起點往窄、每個寬度都合格的最窄寬度。
   - single：根元素 `data-widget` 改名，讓收合 media query 不生效，清單列一直維持單行。從設計寬往窄掃，原始判準的 N 就是「單行開始重疊」的寬度。斷點＝N 取整＋4（D5「單行版面開始重疊的寬度＋餘裕」），且不大於設計寬。
   - collapsed：widget.css 原樣，從斷點下方往窄掃。含可讀性下限的 N 取整＋4＝`min_width`（D5「取整加 4」）。
   - actual：widget.css 原樣，從設計寬往窄掃，用來核對整段寬度。
5. 字型：widget.css 字型堆疊、Noto Sans TC、Microsoft JhengHei、system-ui、sans-serif 各量一次（使用者機器未必裝 Noto Sans TC），取最大值。

重跑：

```text
node host/tests/compare/measure-list-collapse.mjs          # 文字摘要
node host/tests/compare/measure-list-collapse.mjs --json   # 完整結果
```

## 最差情況字串

| 小工具 | 內容 |
| --- | --- |
| 總經日曆 | `update_tw_events.py` 中譯字典 `T` 的全部譯名（執行時解析），加上字典查不到、保留英文的標題：fixture 實例「Final GDP Price Index (季增)」「FOMC Member Waller 談話」，以及依 ForexFactory 常見英文標題自擬的 7 則（含最長的不可斷單字 Manufacturers、Expectations、Productivity、Supervision；這幾則不是逐字查證的來源字串，只用來放大單行版面的最壞情況）。全部 `impact: high`（標題粗體較寬），每 7 筆有 1 筆沒有數值，其餘「預 -102.3B｜前 -102.3B」（7 字元，ForexFactory 常見的最長格式）。共 103 列。 |
| 台股固定事件 | 列由日期計算、不吃資料：今天設在 2026-12-18（季結算日，有「今天」徽章），渲染後再附加最差組合——日期「12/28 (一)」，標籤「台股・那指・道瓊期貨季度結算」「12月營收公布截止（10日前）」「Q4＋年報 財報公布截止」「台指期／選擇權結算」，各有帶「今天」徽章的本週列與「下一次」列兩種。 |
| 台股動態事件 | 今天 2026-12-28。12/28–12/31 每天的財報、法說會、股東會各類，代號＋名稱「00985B 群益ESG投等債0-5」（fixture 實例中最長）、「7631 聚賢研發-創」「2330 台積電」，備註「股東臨時會・改選董監」「Q2 財報（線上）」「法說會（線上）」等；今天的列帶「今天」徽章。處置股為上櫃第 2 次「櫃・第2次・至 12/31」。 |
| 擴充插槽 | 標題「擴充插槽 custom1」，副標題「更新於 2026/12/28 23:58:58」，摘要換成含長英數鍵名的字串（customN.json 可放任意 JSON）。 |

除權息預設不顯示，未納入量測。「除權息」chip 與「股東會」同為三個字，備註（如「現金0.221074元」）短於「股東臨時會・改選董監」，收合後數值獨占一行，不影響結論。

## 各清單量測值（CSS px，五種字型取最大）

| 小工具 | 單行原始 N | 單行含下限 N | 收合原始 N | 收合含下限 N | 斷點 | `min_width` |
| --- | --- | --- | --- | --- | --- | --- |
| 總經日曆 | 419 | 419 | 182 | 225 | 423 | 229 |
| 台股固定事件 | 243 | 270 | 243 | 270 | 無 | 274 |
| 台股動態事件 | 463 | 469 | 284 | 329 | 467 | 333 |
| 擴充插槽 | 153 | 235 | 153 | 235 | 無 | 239 |

各字型的差異都在 5 px 以內。總經日曆的單行 N：widget.css 字型堆疊與 Noto Sans TC 為 416，Microsoft JhengHei 與 system-ui 為 419，sans-serif 為 414；其他小工具各字型相差 0–1 px。

第一個不合格的原因：

- 總經日曆：單行版面在 415 時，「Tankan Large Manufacturers Index」列的 `.n` 被擠出列外（標題不可斷單字的最小寬撐住）。收合版面在 224 時，「非農就業人數」列的標題寬 57 px，小於 4em，觸發 R1。
- 台股固定事件：單行版面在 242 時，「12月營收公布截止（10日前）」列的「今天」徽章超出列；在 269 時「台股・那指・道瓊期貨季度結算」標題寬 59 px，小於 4em。它沒有右側數值，收合與單行版面相同，所以不設斷點，`min_width` 取單行含下限的值。
- 台股動態事件：單行版面在 462 時，「股東會 00985B 群益ESG投等債0-5／股東臨時會・改選董監」列的 `.n` 超出列。也就是說，最差組合在設計寬 470 下已經只剩 8 px 餘裕，463–468 之間標題已小於 4em（現行版面原本就如此）。收合版面在 328 時，標題寬 59 px，小於 4em。
- 擴充插槽：233–234 時 `.ttl`「擴充插槽 custom1」換行，觸發 R3；只看原始判準可窄到 153。

## 未達目標的原因

台股動態事件收合後第一行固定佔用：日期 `.d`（`min-width: 86px`）、chip 約 54、「今天」徽章約 39、四個 gap 共 40，再加上 R1 的標題 4em（60），合計約 279。加上列內距 18、清單內距 24、面板框線 2、`--widget-gap` 16，即約 339，與實測 329＋4 相符。只看原始判準（標題可窄到一個字）也要 288，仍大於 0.55 × 470＝258.5。

要壓到目標以下，得改動第一行的內容：例如把「今天」徽章移到第二行，或在收合時縮小日期欄的 `min-width`。前者違反 spec「時間、標記與標題維持在第一行」，後者會讓各列標題不對齊，因此本 task 不做，留給使用者決定。台股固定事件同理：日期 86＋徽章 39＋標題 4em。

## 核對

都由同一支腳本在最終數值下執行：

- 設計寬下，actual 版面與改動前版面（`data-widget` 改名，並撤銷本 task 不限寬度的兩條規則）逐元素矩形相同。四個小工具皆 `true`。
- 斷點兩側：總經日曆在 423 時有數值的 89 列全為單行、422 時全為兩行；台股動態事件在 467 時 66 列全為單行、466 時全為兩行。設計寬下全為單行。
- `min_width` 下（總經 229、固定 274、動態 333、擴充 239），五種字型的原始判準與可讀性下限問題數皆為 0，有數值的列全為兩行。
- 整段寬度（設計寬到 `min_width`）的原始判準都合格；收合範圍（斷點下方到 `min_width`）含可讀性下限都合格。台股動態事件在 467–468 之間仍是單行，標題小於 4em，這是現行版面本來的樣子，不是本 task 造成的。
- spec 情境「總經日曆邏輯寬 0.64 × 500＝320、高度充足、字級 200%」：`contentZoom` 為 1.28（≥ 1.2），CSS 寬 250，問題數 0，89 列全為兩行。spec「可用寬度只剩設計寬一半」（250）即為同一個寬度。

## 測試與品質 gate

- 新增 `host/tests/list-collapse.test.mjs`：
  - widget.html 寫入 `data-widget`。
  - 總經日曆與台股動態事件各恰有一個收合區塊，且含四條規則。
  - 斷點與 `min_width` 釘住上表數值，且 `min_width < 斷點 < 設計寬`。
  - 總經日曆 `min_width ≤ 266`。
  - 台股固定事件與擴充插槽沒有斷點，`min_width` 釘住實測值。
  - 清單 `min_width` 不大於舊值。
- Rust：
  - `zoom_boxes_match_design_d1_table` 改為新數值。
  - 新增 `narrow_macro_box_scales_up_with_double_font`（`layout::content_zoom` 在邏輯寬 0.64 × 設計寬、字級 2.0 下 ≥ 1.2）。
  - `zoom_boxes_keep_comfort_at_least_min` 照常通過。
- 同步更新：
  - `host/tests/visual-edges-geometry.test.mjs` 的 `widgetZoomBoxes()` 期望值。
  - `widget-edit-mode.test.mjs`、`widget-init-race.test.mjs` 的 document 測試樁補上 `documentElement.dataset`。
- 已跑：
  - `cargo fmt --check`
  - `cargo clippy --all-targets`（另跑一次 `--features self-test-ipc`）
  - `cargo test`（1916 passed）與 `--features self-test-ipc`（1942 passed）
  - `node --test host/tests/*.test.mjs`（57 passed）
  - `node host/tests/compare/verify-visual-edges.mjs --assert`（結束碼 0）
- 前端只改 `host/ui/**` 時 cargo 不會重新嵌入；實機驗收（task 5.2）前要 clean rebuild。
