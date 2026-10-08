# 對照測試腳本（design.md D11 / task 4.2）

比較 Lively 版 `finance-calendar.html` 與新版小工具 `host/ui/widget.html?w=<id>`
的顯示結果是否一致，供 4.3–4.5 搬移小工具時當驗收工具。

## 用法

```sh
# 自比對（預設）：Lively 版對同一面板擷取兩次，逐行比較，五個面板全跑
node host/tests/compare/compare.mjs

# 只跑單一面板
node host/tests/compare/compare.mjs --panel clock

# 反例：第二次擷取前故意竄改 clockTime 的文字，證明腳本抓得到差異（FAIL、exit code 1）
node host/tests/compare/compare.mjs --panel clock --inject-diff

# 跨版本比對：Lively 版 vs 新版 widget.html?w=clock（4.3–4.5 搬移對應小工具後這樣驗收）
node host/tests/compare/compare.mjs --widget clock

# 任一模式加 --evidence [名稱]：同一份輸出另寫成 host/tools/evidence/compare-<名稱>.log
#（去識別；省略名稱時依模式命名，例如 widget-clock）
node host/tests/compare/compare.mjs --widget clock --evidence

# 指定固定時間（含時區偏移）＋自訂 fixture，供跨日／休市日／資料過期等情境用
node host/tests/compare/compare.mjs --panel dynamic --today 2026-10-03T09:00:00+08:00
node host/tests/compare/compare.mjs --fixture path/to/other-tw-events.json

# 跨日／節流恢復（spec「跨日更新」；task 4.3）：新版頁面只導覽一次（固定在 --today），
# 不重新載入頁面的情況下把時間換成 --advance-to 並觸發 visibilitychange，比對對象是
# Lively 版在 --advance-to 這個時間點「重新導覽」的結果（Lively 靠整頁重載達成同樣效果）
node host/tests/compare/compare.mjs --widget macro \
  --today 2026-09-28T03:59:00+08:00 --advance-to 2026-09-28T04:01:00+08:00

# 休市日順延（task 4.4）：--today 是休市日或週末時，dynamic 的處置股節會顯示
# 「休市，順延 <下一交易日>」。host/ui/fixtures/tw-events.json 的預設 --today
# （2026-09-28）本身就是 fixture 的休市日之一，標準跨版本比對已經帶到這個情境；下面這個
# 額外案例從週五休市日出發、連跳週末＋另一個週一休市日，落到下週二，驗證多天連續非交易日
# 的順延邏輯：
node host/tests/compare/compare.mjs --widget dynamic --today 2026-09-25T09:00:00+08:00

# 「顯示除權息」設定（specs/finance-widgets；task 4.4）：獨立腳本，見下方「驗證『顯示
# 除權息』設定」一節
node host/tests/compare/verify-dividend-setting.mjs

# 行情條跑馬燈暫停/恢復＋無資料時高度為 0（task 4.5）：獨立腳本，見下方「驗證行情條暫停/
# 恢復與無資料高度」一節
node host/tests/compare/verify-quotes-pause.mjs

# 視窗邊緣視覺檢查（visual fix）：以實機格子尺寸透明截圖，四邊最外 1px alpha 須為 0、
# 各小工具面板到視窗邊緣的距離＝round(8 × 縮放比例 × 該小工具倍率) ±1 實體 px；截圖存 host/tools/evidence/visual-fix-*-<label>.png
node host/tests/compare/verify-visual-edges.mjs --label after --assert

# 設定視窗：開啟小工具找不到空位時顯示訊息並還原開關（task 7.4）：獨立腳本，比對對象是
# settings.html 本身（沒有 Lively 版可對照），見該檔頭註解
node host/tests/compare/verify-settings-widget-toggle-reject.mjs
```

`--help` 看完整選項。所有選項與行為見 `compare.mjs` 檔頭與各函式上方的中文註解，不重複抄一份
在這裡（唯一來源在程式碼裡，README 只講「怎麼跑、為什麼這樣設計」）。

## 面板 ↔ 小工具對照

比對單位定義在 `panels.mjs`：clock／macro／fixed／dynamic／quotes 五個 id，各自對應
Lively 版一個面板的 CSS 選取器與新版 `#widget-root`，並各自列了排除清單（▴▾ 捲動按鈕、
時鐘卡的版本標記 `#verTag`）。4.3–4.6 若新增或調整小工具，這個檔案是唯一要改的地方。

## 零安裝方案的選擇

全程只用 Node 22 內建能力：

- CDP（Chrome DevTools Protocol）驅動 headless Edge：全域 `fetch`（叫 `/json/new` 等
  HTTP 端點）＋全域 `WebSocket`（連 CDP 的 devtools websocket）。`node -e "console.log(typeof
  WebSocket, typeof fetch)"` 實測（2026-09-28，Node v22.19.0）皆為 `function`，不需要
  `--experimental-*` 旗標、不用裝 `puppeteer`／`ws` 等套件。
- 本機靜態伺服器：`node:http`，同一支腳本、同一個語言，不用另外呼叫 `python -m
  http.server`（brief 允許 Python stdlib／Node 內建 WebSocket／PowerShell
  ClientWebSocket 三選一，選 Node 是因為「開瀏覽器」與「開伺服器」可以在同一支腳本、
  同一個事件迴圈裡完成，不必讓兩個語言的行程互相等待、對暫存路徑）。

沒有裝任何 npm 套件、沒有 `package.json`（`.mjs` 副檔名本身就決定用 ES modules，不依賴
`package.json` 的 `"type"` 欄位）。

## 運作方式摘要

1. `serve.mjs` 在暫存目錄組一個伺服器根目錄：`/lively/`（複製一份
   `finance-calendar.html`＋依 fixture 產生的 `tw_events.js`，不動 repo 根目錄）、
   `/new/`（直接讀 `host/ui/` 本體，不複製，永遠是最新版）、`/fixtures/`（複製
   `host/ui/fixtures/` 並用 `--fixture` 覆寫 `tw-events.json`，新版頁面用
   `?fixtures=/fixtures/` 指過來，確保兩邊吃同一份資料）。
2. `cdp.mjs` 啟動一個獨立 `--user-data-dir` 的 headless Edge（`--remote-debugging-port=0`，
   實際埠號從 `<user-data-dir>/DevToolsActivePort` 讀，比自己找可用埠再指定可靠）。
3. 每次擷取開一個新分頁（`PUT /json/new`），直接連該分頁自己的 devtools websocket
   （不走 `Target.attachToTarget` 的 flatten session，少一層間接）：
   - `Emulation.setTimezoneOverride({timezoneId:'Asia/Taipei'})` 固定時區。
   - `Page.addScriptToEvaluateOnNewDocument` 注入一段覆寫 `Date`（零參數 `new Date()`／
     `Date.now()` 一律回傳固定時刻；帶參數呼叫原樣放行，見 `compare.mjs` 該函式上方註解
     為什麼不能全部夾死）＋設定 `window.__TEST_TODAY` 的腳本。
   - `Page.navigate` 到目標頁面，輪詢頁面條件（Lively 版等 `window.TW_EVENTS` 出現；
     新版等 `#widget-root` 有子節點）確認渲染完成，而不是等 `load` 事件（資料是同源
     `<script>` 動態插入，`load` 事件不保證涵蓋它）。
   - `Runtime.evaluate` 執行擷取腳本：clone 選取到的面板節點、移除排除清單命中的子節點、
     依 DOM 順序走訪所有文字節點，空白正規化＋trim，回傳非空字串陣列。
   - 關閉分頁（`GET /json/close/<id>`）。
4. 逐行比較兩次擷取的陣列，不同就列出來（index 對齊，最多印 30 處）。

## 驗證「顯示除權息」設定（task 4.4）

`compare.mjs` 的標準跨版本比對只驗證「兩邊都不顯示除權息」時一致（`show_dividend:false`，
與 Lively 版預設值相同）；它不支援臨時切換新版 fixture 的 `settings.json`、也沒辦法在
Lively 版注入 `CONFIG.dynTypes` 的修改。獨立腳本 `verify-dividend-setting.mjs` 專門驗證
「打開之後」的行為：把新版 fixture 的 `settings.json` 覆寫成 `show_dividend:true`，同時把
Lively 版的 `CONFIG.dynTypes` 直接設成與 `dynamic.js` 的 `dynTypesFor({show_dividend:
true})` 相同的陣列（不是 `push` 到陣列尾端——真實 fixture 有多筆除權息與財報／股東會／
法說會同一天，順序不同會讓排序 tie-break 不同，比對才有意義），重新呼叫
`renderDyn(window.TW_EVENTS)`，逐行比對兩邊輸出。不重複造一套 CDP／伺服器骨架，`compare.mjs`
的 `taipeiDateStr`／`buildOverrideScript`／`buildExtractScript` 抽到零副作用的
`capture-utils.mjs` 供兩支腳本共用（`compare.mjs` 檔尾會在被 `import` 時觸發自己的
`run()` 並 `process.exit()`，不能被其他腳本直接 `import`）。

```sh
node host/tests/compare/verify-dividend-setting.mjs
```

## 驗證行情條暫停/恢復與無資料高度（task 4.5）

`compare.mjs` 的兩種模式（自比對／跨版本比對）都是「兩邊都跑同一段邏輯、逐行比較文字」，
沒辦法表達「暫停」這件事——Lively 版本來就沒有暫停的概念（`quotes.js` 檔頭「暫停/恢復」一節
有完整說明），沒有對應的 Lively 行為可以當比較基準；也沒辦法表達「讀
`window.__bridgeTest.lastReportContent`」這種非文字斷言。因此另開
`verify-quotes-pause.mjs`：只開新版 `widget.html?w=quotes` 頁面，用 `bridge.js` 的測試
掛鉤（`window.__bridgeTest.emit('pause', {...})`，`hasTauri===false` 時才存在，見
`bridge.js` 檔尾）模擬宿主的暫停/恢復通知，直接讀 `.tlist` 的 `scrollLeft` 斷言：

1. 跑馬燈啟動後一段時間 `scrollLeft` 真的在變化（前提檢查，避免「本來就不動」造成的假陽性）。
2. 送 `{paused:true}` 後一段時間 `scrollLeft` 不再變化。
3. 送 `{paused:false}` 後 `scrollLeft` 從暫停當下的值繼續前進（不是歸零重新開始）。

另外用 `host/tests/compare/fixtures/tw-events-empty-quotes.json`（複製自
`host/ui/fixtures/tw-events.json`，只把 `quotes` 改成 `[]`）開另一個伺服器，驗證
`window.__bridgeTest.lastReportContent` 為 `false`、`container.style.display` 為 `none`
（design.md D7：無內容時宿主隱藏視窗；task 7.3 起 IPC 改叫 `report_content(hasContent)`，本
腳本只驗前端回報的值本身正確，套用端行為在 host 端的 Rust 測試與實機驗收）。

```sh
node host/tests/compare/verify-quotes-pause.mjs
```

## 已知限制 / 留給後續 task 的提醒

- **`--widget <id>` 目前 `clock`／`macro`／`fixed`／`dynamic`／`quotes` 會真的比對出東西**
  （task 4.3–4.5 已搬移）：`custom1`–`custom5` 尚未搬移（task 4.6–4.7），`widget.html`
  會顯示「小工具尚未實作」佔位訊息，`--widget custom1` 這類指令現在跑起來一定是 FAIL——這是
  預期中的 FAIL，不是腳本本身的 bug；等對應 task 搬完小工具後，這條指令應該轉為 PASS，
  才是該 task 的驗收證據。
- **除權息開關差異（D11 明列的刻意差異）不是用排除選取器處理的**：Lively 版
  `CONFIG.dynTypes` 預設不含 `'dividend'`，`host/ui/fixtures/settings.json` 的預設設定
  也是關閉，兩邊預設值相同時本來就不會出現除權息項目，不需要額外處理；但如果之後測試
  改成兩邊設定不同（例如新版設定打開除權息、Lively 版沒開），`dynamic` 面板的比對會抓到
  差異，這是**設定不同步造成的真差異**，不是本腳本要吸收的雜訊——遇到這種情況應該檢查
  兩邊設定是否真的同步，而不是加選取器排除掉。
- **`quotes` 面板在視窗夠寬時內容會被複製一份**（`finance-calendar.html`
  `renderQuotes()`／`setupAutoScrollX()`：超寬時複製第二份內容做頭尾相接跑馬燈，見該函式
  註解），本腳本目前用固定 headless 視窗大小，兩次擷取的視窗寬度相同、複製行為一致，
  自比對不受影響。**task 4.5 用 `--widget quotes` 實測跨版本比對確實 PASS**
  （`host/tools/evidence/4.5-widget-quotes-cross-compare.log`）。fix F6 起 `quotes.js` 不再把
  container 的 CSS 寬度寫死成 992px（`host/src/widgets.rs` `WIDGET_SPECS` 中 quotes 的設計寬度），
  面板隨視窗寬度扣掉左右 `--widget-gap` 填滿（倍率被夾住時也一樣），跑馬燈判斷跟著實際面板寬度
  走；13 檔行情的總內容寬度遠超過任何常見的 headless 視窗寬度
  （不論 Lively 側被 grid 的 `minmax()` 擠壓到多窄），兩邊都會判定「超寬」而觸發複製，
  複製後的文字內容因此逐行相同。若之後 fixture 的行情筆數大幅減少到接近臨界值（複製與否
  剛好卡在邊界），才需要重新檢視這個假設；目前不建議額外加正規化去吸收「多／少一份重複
  內容」這種差異——那仍然是合理訊號，代表兩邊行為真的不同。
- **「已 N 天未更新」過期警示只在 `dynamic` 面板（`#dynFoot`），不在 `macro`
  （`#macroFoot`）**：逐行核對 `finance-calendar.html` 456–512 行（`renderMacro`）確認
  `macroFoot` 全程只用 `data.updated`，完全沒有過期判斷；過期警示邏輯在 678–687 行
  （`renderDyn`）用 `data.fetched` 算天數。`host/tests/compare/fixtures/tw-events-stale-3d.json`
  （複製自 `host/ui/fixtures/tw-events.json`，只把 `fetched` 改成 3 天前）用來實測
  （`host/tools/evidence/4.3-footer-ownership-macrofoot-vs-dynfoot.log`）確認：`macroFoot`
  仍只顯示「資料更新：…」，`dynFoot` 才出現「⚠ 已 3 天未更新」。**task 4.4 搬移 `dynamic`
  小工具時，過期警示是該 task 的搬移範圍，不是 4.3 的**；task 4.4 對照測試建議直接沿用
  `tw-events-stale-3d.json`（`--fixture host/tests/compare/fixtures/tw-events-stale-3d.json`）
  驗證有搬對，不需要另外做一份。
- 擷取條件滿足後另外等了固定 200ms（`compare.mjs` `captureOnce()`），是給 `renderXxx()`
  實際寫入 DOM 的緩衝，本機實測綽綽有餘；若之後在明顯較慢的機器上跑出 flaky 結果，
  優先調整這個常數或改良等待條件，而不是加重試迴圈掩蓋。
- headless Edge 的收尾曾踩到一個坑：`proc.kill()` 送出終止訊號後立刻回傳，但 Windows
  對「行程仍持有 `--user-data-dir` 檔案控制代碼」的鎖定比 POSIX 嚴格，緊接著 `rm()`
  常因為程序還沒真正結束而失敗；已改成等 `exit` 事件＋`rm` 失敗時重試（`cdp.mjs`
  `launchEdge().close()`），實測（`Get-CimInstance Win32_Process` 核對＋暫存目錄計數）
  收尾後沒有殘留的 headless 行程或暫存目錄。
