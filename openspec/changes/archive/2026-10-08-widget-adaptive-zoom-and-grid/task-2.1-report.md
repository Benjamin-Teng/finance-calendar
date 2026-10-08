# Task 2.1 報告：時鐘內容自然寬度量測

來源：`openspec/changes/widget-adaptive-zoom-and-grid/tasks.md` 2.1；設計依據 `design.md` D1。

## 結論

| 項目 | 數值 |
| --- | --- |
| 實測值（整扇視窗所需邏輯寬度，含左右各一個 `--widget-gap`） | **207.625** CSS px |
| 不小於實測值的整數 | 208 |
| 餘裕（design.md Risks） | +4 |
| **建議 `min_width`（時鐘 `WIDGET_SPECS` 設計寬度）** | **212** |
| 最寬組合下視窗所需高度（面板自然高 139.1875 ＋ 上下 gap 16） | 155.1875，≤ 156，**高度不需調整** |

要點：

- 三行文字裡最寬的是**時間**（52px 粗體、`letter-spacing:2px`、`tabular-nums`），所有 1440 個
  `HH:MM` 寬度完全相同（191.625，等寬數字），由時間行決定面板寬；日期行最寬 165.375、週別行
  最寬 159.625，都不到時間行，不影響結論。
- `--widget-gap` 確實在左右兩側各一個：`#widget-root` 的 `padding: var(--widget-gap)`（四邊
  各 8），所以視窗寬＝面板 border box 寬＋16。
- 面板自身 chrome：`padding: 14px 20px 16px`、`border: 1px`、`box-sizing: border-box`，面板寬＝
  內容寬 149.625＋左右 padding 40＋左右 border 2＝191.625。
- 高度：同一最寬內容下面板自然高 139.1875（時間行 57.1875、日期行 21、週別行 19、加 margin 與
  padding），加上下 gap 共 155.1875，比現行 `design_min_height`＝140＋16＝156 少 0.8125，**不必
  調整**。餘裕很小（不到 1px），若之後動到時鐘字級或 padding 要重跑本腳本。

## 量測方法

腳本：`host/tests/compare/measure-clock-natural-width.mjs`（零安裝，靠 Node 22 內建 CDP，沿用
`serve.mjs`／`cdp.mjs`；headless Edge、獨立 `--user-data-dir`、用完即刪）。重跑：

```sh
node host/tests/compare/measure-clock-natural-width.mjs          # 人類可讀輸出
node host/tests/compare/measure-clock-natural-width.mjs --json   # 完整 JSON
```

步驟：

1. 以 CDP `Emulation.setDeviceMetricsOverride` 設 `deviceScaleFactor=1`、不套任何 ZoomFactor
   （倍率 1），開 `widget.html?w=clock`（fixture 模式）。枚舉用視窗 1400x600，避免窄視窗換行污染。
2. 注入樣式 `#widget-root > .panel { flex:none; width:max-content; align-self:flex-start;
   height:auto }`，面板依內容收縮；以 `getBoundingClientRect()` 讀 border box 寬高（含小數）。
3. 候選枚舉（單看一行時，其餘兩行清空，量面板寬）：
   - 時間：`00:00` 至 `23:59` 共 1440 個字串。
   - 日期：2026-01-01 至 2031-12-31 每天的 `clock.js` 格式 `Y/M/D　週X`，去重後 2191 個。
   - 週別：同一區間每天的 `本週 M/D – M/D`（呼叫 `common.js` 真實的 `weekStartOf`／`addDays`／
     `md`），去重後 313 個。
4. 取三行各自最寬字串組合後再量一次面板寬高，加 `#widget-root` 左右 gap 即視窗所需寬高。
5. 交叉核對：以 `capture-utils.mjs` 的 Date 覆寫固定在 2026/12/28 23:58，走 `clock.js` 真實
   `tick()` 路徑，量到相同的面板寬高（見下表）。
6. 正式版面核對：視窗（viewport）設成「W x 156」不動 CSS，面板由 `flex:1` 填滿，檢查
   `scrollHeight`／`clientHeight`、`scrollWidth`／`clientWidth` 是否有裁切。
7. 字型敏感度：強制 `body` 改用其他字型，量同一組最寬字串。

## 實際生效字型

`CSS.getPlatformFontsForNode` 與 computed style（量測當下，本機）：

| 元素 | 平台實際字型 | font-size／weight | letter-spacing |
| --- | --- | --- | --- |
| `#clockTime` | Noto Sans TC | 52px／700（`tabular-nums`） | 2px |
| `#clockDate` | Noto Sans TC | 15px／400 | normal |
| `#weekRange` | Noto Sans TC | 13px／400 | 0.5px |

CSS 宣告：`"Noto Sans TC", "Microsoft JhengHei", "PingFang TC", system-ui, sans-serif`。本機
有裝 Noto Sans TC，所以生效的是第一順位。

## 候選字串寬度表（面板 border box 寬，CSS px）

| 行 | 字串 | 寬度 |
| --- | --- | --- |
| 時間 | `00:00`、`23:58`、`11:11`、`12:30` 與其餘 1440 個全部 | 191.625（範圍 min＝max） |
| 日期 | `2026/10/10　週六`（最寬，與 10/11 至 10/14 等並列） | 165.375 |
| 日期 | `2026/12/28　週一` | 165.375 |
| 日期 | 最窄 | 148.719 |
| 週別 | `本週 10/11 – 10/17`（最寬，與 10/18 至 10/24 等並列） | 159.625 |
| 週別 | `本週 12/27 – 1/2` | 144.188 |
| 週別 | 最窄 | 128.766 |

最寬組合（時間 `00:00`＋日期 `2026/10/10　週六`＋週別 `本週 10/11 – 10/17`）：

| 項目 | 數值 |
| --- | --- |
| 面板寬 | 191.625 |
| 面板高 | 139.1875 |
| 視窗寬（面板＋左右 gap） | 207.625 |
| 視窗高（面板＋上下 gap） | 155.1875 |
| 真實 tick 路徑（Date 固定 2026/12/28 23:58）面板寬／高 | 191.625／139.1875（與上列一致） |

## 正式版面核對（視窗 W x 156，最寬字串）

| 視窗寬 | 面板 clientW／scrollW | 面板 clientH／scrollH | 是否裁切 |
| --- | --- | --- | --- |
| 212（建議） | 194／194 | 138／138 | 否 |
| 208（實測值取整） | 190／190 | 138／138 | 否 |
| 207（取整再少 1） | 189／189 | 138／138 | 否（時間行 `scrollW` 150 > `clientW` 149，已開始溢出約 1px） |

結論：207 已開始有 1px 內容溢出，208 剛好塞得下，與實測值 207.625 吻合；建議 212 留 4px 給字型
差異。

## 字型敏感度（強制 body font-family，最寬字串）

使用者機器若沒有 Noto Sans TC，會退到下一順位。同一組字串的量測：

| 強制字型 | 平台實際字型 | 面板寬 | 視窗寬 | 視窗高 |
| --- | --- | --- | --- | --- |
| Noto Sans TC | Noto Sans TC | 191.625 | 207.625 | 155.1875 |
| Microsoft JhengHei | 微軟正黑體 | 189.141 | 205.141 | 152.1875 |
| system-ui | Microsoft JhengHei UI | 189.141 | 205.141 | 150.1875 |
| sans-serif | Arial | 185 | 201 | 155.1875 |

各退化字型下的視窗寬、高都不超過 Noto Sans TC 的結果，所以以 Noto Sans TC 為準的 212 夠用。
正黑體下日期行（面板寬 169.06）與週別行（163.89）雖比 Noto Sans TC 寬，仍小於同字型下時間行
的 189.14，寬度仍由時間行決定。

## 量測指令的實際輸出節錄

```text
== 實際生效字型 ==
{ "platform": { "clockTime": ["Noto Sans TC x5"], "clockDate": ["Noto Sans TC x12"],
  "weekRange": ["Noto Sans TC x15"] }, ... }

== 枚舉數量 ==
{"times":1440,"dates":2191,"weeks":313}

== 面板 chrome ==
{"panelPadding":"14px 20px 16px","panelBorder":"1px","boxSizing":"border-box","rootPadding":[8,8,8,8]}

== 最寬組合 ==
{"worst":{"time":["00:00",191.625],"date":["2026/10/10　週六",165.375],"week":["本週 10/11 – 10/17",159.625]},
 "combined":{"panelW":191.625,"panelH":139.188,"windowW":207.625,"windowH":155.188}}

== 真實 tick 路徑（Date 固定 2026-12-28 23:58）==
{"text":["23:58","2026/12/28　週一","本週 12/27 – 1/2"],"panelW":191.625,"panelH":139.1875}

== 結論 ==
{"measuredWindowW":207.625,"ceilW":208,"safetyPx":4,"minWidth":212}
高度： {"naturalPanelH":139.1875,"naturalWindowH":155.1875,"currentDesignMinHeight":156,"withinCurrent":true}
```

## 給下一個 task 的提醒

- 時鐘 `WIDGET_SPECS` 的設計寬度由 500 改為 **212**（視窗寬，已含左右 gap）；設計最小高度維持
  `140 + 2 * WIDGET_GAP_CSS_PX`＝156。
- 高度餘裕只有 0.8125 px（Noto Sans TC 下）；`#clockTime` 的 `line-height:1.1` 與
  `font-size:52px` 是主要來源，若日後調整時鐘樣式需重跑本腳本。
- 本量測不含 `body.edit-mode` 的外框與把手，它們畫在視窗內側，不影響內容寬度。
