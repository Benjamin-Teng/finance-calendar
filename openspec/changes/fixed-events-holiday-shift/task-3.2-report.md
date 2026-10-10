# Task 3.2 報告：台股固定事件最小框寬重量

## 結論

台股固定事件的最小框寬由 **274 改為 285**（`host/src/widgets.rs` 的 `WIDGET_SPECS`）。

| 項目 | 改動前 | 改動後 |
|---|---|---|
| 最窄合格寬（原始判準＝可讀性下限，五種字型取最大） | 見 widget-font-scale-per-widget task-3.1-report | 281 |
| `min_width`（取整＋4） | 274 | **285** |
| 收合斷點 | 無（標題在列內換行） | 無 |

## 原因

順延徽章「原 M/D」放在標題 `<b>` 內、設 `white-space: nowrap`（design.md D3），標題欄位的最小內容寬因此由一個全形字變成徽章寬度（「原 12/31」約 66px）。同一列又有「今天」徽章時（例如 2026-11-16：Q3 財報 11/14 順延到當天），寬度 274 下「今天」徽章超出列（量測 `atMinWidth` 4 個 raw 問題，首例「列『12/28 (一)那指・道瓊期貨季度結算原 12』的 `span.badge.today` 超出列」）。

## 量測方式

`node host/tests/compare/measure-list-collapse.mjs`（2026-10-10，本機 headless Edge）。最差情境 `FIXED_WORST_ROWS` 改為：四個標籤（季結算改為「那指・道瓊期貨季度結算」）各三列——本週列帶順延徽章＋今天徽章、本週列只帶今天徽章、下一次列帶順延徽章；順延徽章用最寬的「原 12/31」。

固定事件量測輸出（字型堆疊）：

```text
結論： {"singleRawOk":281,"singleReadOk":281,"collapsedRawOk":281,"collapsedReadOk":281,"actualRawOk":281,"actualReadOk":281,"suggestedBreakpoint":285,"suggestedMinWidth":285}
目前數值： {"cssBreakpoint":null,"minWidth":274}
```

台股固定事件沒有右側數值、不設收合斷點，`suggestedBreakpoint` 不適用。總經日曆、台股動態事件與擴充插槽的量測結論與 `widget-font-scale-per-widget` task-3.1-report 相同（229／333／239），未改動。
