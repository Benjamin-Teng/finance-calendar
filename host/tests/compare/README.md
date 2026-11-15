# 小工具畫面的基準檔比對與驗證腳本

`compare.mjs` 在固定時區、固定時間、固定資料下開 `host/ui/widget.html?w=<id>`，把畫面上的文字逐行擷取出來，
與 `golden/<情境>.txt` 逐行比較。同目錄的 `verify-*.mjs`、`measure-*.mjs` 是各自獨立的驗證與量測腳本，共用這裡的
伺服器與 CDP 模組。

## 用法

```sh
# 全部情境
node host/tests/compare/compare.mjs

# 只跑某個小工具的情境、或單一情境
node host/tests/compare/compare.mjs --widget macro
node host/tests/compare/compare.mjs --scenario dynamic-dividend

# 列出情境
node host/tests/compare/compare.mjs --list

# 反例：擷取前在畫面上多塞一行，證明比對抓得到差異（FAIL、結束碼 1）
node host/tests/compare/compare.mjs --scenario clock --inject-diff

# 刻意改了顯示：重產基準檔，然後在 git diff 裡把改動讀過一遍再提交
node host/tests/compare/compare.mjs --widget dynamic --update

# 加 --evidence [名稱]：同一份輸出另寫成 host/tools/evidence/compare-<名稱>.log（去識別）
node host/tests/compare/compare.mjs --evidence
```

`--help` 看完整選項。結束碼：0＝全部相同，1＝有差異或缺基準檔，2＝執行錯誤。

## 情境與基準檔

情境清單在 `scenarios.mjs`，一個情境＝一個小工具在某個時間、某份資料、某組設定下的畫面。目前涵蓋五個小工具的預設
畫面，加上跨日、跨週後不重新載入的重算、連續休市日的處置股順延、資料 3 天未更新的頁尾、沒有行情資料、打開「顯示除權息」。

新增情境：在 `scenarios.mjs` 加一筆，跑 `--scenario <name> --update`，讀過產生的基準檔再提交。

基準檔的來歷：Lively 版在 2026-11-16 從 repo 移除（change `retire-lively`）。移除前，總經日曆、台股動態事件、行情條的
每個情境都先確認與 Lively 版逐行相同才存成基準檔；例外是 `dynamic-stale-3d` 的頁尾警示文案，v0.3.6 刻意改寫過。
時鐘與台股固定事件在更早的 change 就刻意與 Lively 版不同，基準檔是當時的新版輸出。

基準檔只記文字。顏色（例如時鐘的開收盤）與版面由各自的單元測試與量測腳本負責：
`host/tests/exchange-clock.test.mjs`、`host/tests/fixed-holiday-shift.test.mjs`、`measure-clock-natural-width.mjs`、
`measure-list-collapse.mjs`。

## 運作方式

1. `serve.mjs` 在暫存目錄組一個伺服器根目錄：`/new/`（直接讀 `host/ui/` 本體，不複製，永遠是最新版）、
   `/fixtures/`（複製 `host/ui/fixtures/` 並用情境指定的 fixture 覆寫 `tw-events.json`，頁面用
   `?fixtures=/fixtures/` 指過來）。情境有 `settings` 時，`compare.mjs` 改寫暫存目錄裡的 `settings.json`，不動 repo。
2. `cdp.mjs` 啟動一個獨立 `--user-data-dir` 的 headless Edge（`--remote-debugging-port=0`，實際埠號從
   `<user-data-dir>/DevToolsActivePort` 讀）。
3. 每個情境開一個新分頁，直接連該分頁自己的 devtools websocket：
   - `Emulation.setTimezoneOverride({timezoneId:'Asia/Taipei'})` 固定時區。
   - `Page.addScriptToEvaluateOnNewDocument` 注入覆寫 `Date` 的腳本（零參數 `new Date()`／`Date.now()` 回傳固定時刻，
     帶參數呼叫原樣放行，理由見 `capture-utils.mjs`）並設定 `window.__TEST_TODAY`。
   - `Page.navigate` 後輪詢 `#widget-root` 有子節點，確認渲染完成。
   - 情境有 `advanceTo` 時，不重新導覽，直接把 `Date` 換成新的時間並送 `visibilitychange`，驗證「恢復後立即重算」。
   - `Runtime.evaluate` 擷取：clone `#widget-root`、移除 `.edit-placeholder`、依 DOM 順序走訪文字節點，空白正規化後
     回傳非空字串陣列。
4. 與基準檔逐行比較，不同就列出來（最多印 30 處）。

全程只用 Node 22 內建能力（全域 `fetch`／`WebSocket`、`node:http`），沒有 npm 套件、沒有 `package.json`。

## 其他腳本

```sh
# 行情條跑馬燈暫停／恢復，以及沒有行情資料時回報無內容
node host/tests/compare/verify-quotes-pause.mjs

# 視窗邊緣視覺檢查：四邊最外 1px alpha 為 0、面板到視窗邊緣的距離符合倍率；截圖存 host/tools/evidence/
node host/tests/compare/verify-visual-edges.mjs --label after --assert

# 設定視窗：開啟小工具找不到空位時顯示訊息並還原開關
node host/tests/compare/verify-settings-widget-toggle-reject.mjs
```

其餘 `verify-*`、`measure-*`、`regression-*` 的用途與用法寫在各檔檔頭。

`verify-quotes-pause.mjs` 用 `bridge.js` 的測試掛鉤（`window.__bridgeTest.emit('pause', {...})`，沒有 Tauri 時才存在）模擬
宿主的暫停與恢復，讀 `.tlist` 的 `scrollLeft`：啟動後在變化、暫停後不變、恢復後從暫停當下的值繼續。另以
`fixtures/tw-events-empty-quotes.json` 驗證 `window.__bridgeTest.lastReportContent` 為 `false`、容器 `display` 為 `none`。

## 已知限制

- **行情條在視窗夠寬時內容會複製一份**做頭尾相接的跑馬燈。13 檔行情的總寬度遠超過 headless 視窗，所以基準檔裡是
  兩份；行情筆數大幅減少到接近臨界值時，基準檔會跟著變，這是合理訊號，不要加正規化去吸收。
- **「已 N 天未更新」只在台股動態事件的頁腳**，總經日曆頁腳只顯示資料更新時間（`dynamic-stale-3d`、`macro-stale-3d`
  兩個情境各驗一邊）。
- 擷取前固定等 200ms，是給模組寫入 DOM 的緩衝；在明顯較慢的機器上出現不穩定結果時，優先調整這個常數或改良等待條件，
  不要加重試迴圈掩蓋。
- headless Edge 收尾：`proc.kill()` 送出訊號後立刻回傳，Windows 上行程還握著 `--user-data-dir` 的檔案時 `rm()` 會失敗；
  `cdp.mjs` 的 `launchEdge().close()` 會等 `exit` 事件並在刪除失敗時重試。
