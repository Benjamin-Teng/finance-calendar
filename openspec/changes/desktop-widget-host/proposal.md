## Why

財經日曆目前跑在 Lively Wallpaper：Lively 把 WebView2 視窗 `SetParent` 進 explorer.exe 的
WorkerW，當桌布行程被 Restart Manager 終止或卡住時，explorer 會因跨行程同步呼叫逾時而被
Windows 重啟，桌布隨之消失（2026-08 事件記錄已證實三次）。Lively 也只轉發點擊、不轉發滾輪。
2026-09-27 的 Tauri spike 已實測「獨立頂層視窗＋z-order 置底、不 SetParent」在 explorer 重啟時
完全不受影響，且產出的 exe 不依賴 `VCRUNTIME140.dll`，因此改做自有的桌面小工具宿主。

本 change 是三個子專案中的第一個（宿主）；資料層 Rust 化與安裝發布另開 change。

## What Changes

- 新增 `host/` 桌面宿主程式（Rust／Tauri 2）：常駐系統匣，依設定建立多個桌面小工具視窗。
- 小工具視窗為獨立頂層視窗，z-order 維持在所有一般視窗之下、桌面之上；**不** `SetParent`
  進 explorer 的視窗樹。處理建立當下置底、explorer 重啟後重新置底、Win+D 時保持可見。
- 視窗不搶焦點、不出現在工作列與 Alt+Tab；Windows 11 原生 Acrylic 背景與圓角，不支援或
  失效時使用「純色」模式。
- 把現有單頁儀表板拆成五個可各自開關、拖曳擺放的財經小工具：時鐘、總經日曆、台股固定事件、
  台股動態事件、行情條。版面以每個螢幕工作區的 48×48 格線記錄，位置與大小都按工作區比例，
  小工具互不重疊；編輯版面時可拖曳移動與調整大小，放開對齊格線、重疊就彈回。
- **預留五個擴充插槽**（`custom1`–`custom5`）：各自有空白小工具與對應資料通道，日後接新資料源
  （分點進出、選擇權、匯率、利率、殖利率等）只需產出 JSON 並替換該插槽的顯示模組，核心、
  視窗與設定不需修改。
- 資料以「通道」組織：宿主核心讀取各通道的資料檔（財經日曆＝`tw_events.json`，開發期間讀
  Python 產出的資料夾），以事件推送給訂閱該通道的小工具；小工具只負責渲染。
- 設定集中於 `settings.json`，從系統匣開設定視窗修改；開機自啟、單一執行個體。
- 全螢幕程式、鎖定、螢幕關閉、省電模式時暫停動畫；WebView2 行程故障時自動復原；經 Restart
  Manager 關閉時註冊為可重新啟動。
- 清單改用原生滾輪／觸控板／觸控捲動，移除 ▴▾ 翻頁鈕（僅限新版小工具）。
- `finance-calendar.html`（Lively 版）凍結：保留作為過渡期退路，只修重大 bug，不加新功能。

### Lively 版 `CONFIG` 的去向

| 去向 | 項目 |
|---|---|
| 設定視窗可調 | `panelOpacity`（透明度）、`accentColor`（主題色）、`showClock`／`showQuotes`（改為十個小工具各自開關）、`uiScale`（改為由小工具所佔格數決定大小，內容依寬度等比縮放）、`dynTypes` 是否含 `dividend`（「顯示除權息」勾選）、`dataBase`（改為資料目錄） |
| 固定值 | `textColor`、五種事件標籤色、`macroWidth`／`eventsWidth`（成為小工具的設計寬度，即倍率 1 時的寬度）、`autoScrollPxPerSec`（跑馬燈速度）、`dailyReloadAt`（04:00） |
| 移除 | `bgImage`（背景交給系統桌布）、`blurPx`（由系統 Acrylic 決定）、`reloadHours`／`freshCheckMin`（改由核心輪詢）、`columnAutoScroll`、`autoScrollPauseSec`、`quotesLoop=false` 的來回模式 |

版本號改顯示於設定視窗與系統匣選單。

## Capabilities

### New Capabilities

- `widget-host-windows`: 小工具視窗的桌面層行為——置底、Win+D、焦點、外觀模式、格線版面與互不重疊、
  尺寸、編輯版面（移動與調整大小）、多螢幕與 DPI 定位。
- `widget-data-feed`: 資料通道的讀取、驗證、快照與事件推送契約，含五個擴充通道。
- `widget-host-lifecycle`: 系統匣、開機自啟、單一執行個體、設定持久化、暫停與故障復原。
- `finance-widgets`: 五個財經小工具的顯示內容與互動（沿用現有儀表板的渲染規則、滾輪捲動），
  以及五個擴充插槽小工具的預設行為。

### Modified Capabilities

（無——`openspec/specs/` 目前為空，Lively 版不納入規格。）

## Impact

- 新增：`host/`（Rust crate `fc-host`、`host/capabilities/`、`host/ui/` 前端 ES modules、
  `host/tools/` 驗證腳本）。
- 依賴：`tauri` 2.x（啟用 `tray-icon` feature）、`tauri-build` 2.x、`windows` crate、
  `webview2-com`（版本與 tauri 所用者一致，用於訂閱 WebView2 行程故障）；Tauri 官方 plugin
  （autostart、single-instance）。不引入 Node 或前端打包工具。
- 不變：`update_tw_events.py` 與 `tw_events.json` 格式（本 change 只讀不改）；
  `finance-calendar.html` 行為不變。
- 範圍外：資料抓取移入 Rust（子專案 2）、安裝檔／自動更新／ARM64 建置／著陸頁（子專案 3）、
  擴充插槽的實際資料源與顯示（各自另開 change）。
