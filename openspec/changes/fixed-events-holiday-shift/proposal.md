## Why

台股固定事件目前只依日曆規則算日期（每月第三個週三、每月 10 日、3/31 等），落在休市日或週末時仍顯示原日期，與實際結算日／公告截止日不符。例如 2026-02-18 遇春節，期交所實際最後結算日是 2/23。季結算標籤「台股・那指・道瓊」也有誤：台股季月結算在第三個週三（已含在台指期結算那一筆），第三個週五只有美股商品。使用者 2026-10-10 要求處理 Backlog 的「台股固定事件遇假日順延標示」。

## What Changes

- 台指期／選擇權結算、財報公布截止、月營收公布截止：原日期不是交易日（週末或 `tw_events.json` 的 `holidays`）時，改顯示往後第一個交易日，並加「原 M/D」徽章。
  - 台指期結算依期交所契約規格（「最後交易日若為假日……以其最近之次一營業日為最後交易日」）。
  - 財報與月營收的法條沒有寫順延，屬推定（財報有金管會 2024 年把 3/31 週日的期限寫成 4/1 的實例；月營收比照），徽章附說明「推定順延，以金管會公告為準」。
- 「本週」與「下一次」的歸屬、已過去與「今天」判斷，一律以順延後日期為準。
- 季結算不套用台灣休市日；標籤改為「那指・道瓊期貨季度結算」。
- 台股固定事件改為讀取 `tw-events` 通道的 `holidays`；無資料或缺鍵時只依週末判斷。
- 與 Lively 版的對照不再適用於台股固定事件（列為刻意差異）。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `finance-widgets`：「顯示規則與 Lively 版一致」的刻意差異加入台股固定事件的休市順延與季結算標籤；新增「台股固定事件遇休市順延」需求。

## Impact

- 前端：`host/ui/widgets/fixed.js`（訂閱資料、順延計算、徽章、時間敏感狀態更新的徽章選取器）、`host/ui/common.js`（順延純函式）、`host/ui/widget.css`（順延徽章樣式）。
- 版面：清單列多一個徽章，`host/src/widgets.rs` 的台股固定事件最小框寬需以 `measure-list-collapse.mjs` 重量；`host/tests/list-collapse.test.mjs` 與 `task-3.1-report.md` 的數值同步。
- 測試：新增 `host/tests/fixed-holiday-shift.test.mjs`；`host/tests/compare/panels.mjs`／`compare.mjs` 對台股固定事件的 Lively 對照改為已知差異。
- 文件：`AGENTS.md`（現況與 Backlog）、`host/tests/compare/README.md`。
- 不動 Rust 資料層與 `tw_events.json` 格式；Lively 版凍結不改。
