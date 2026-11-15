# Tasks

> 執行路徑：SDD｜理由：跨後端排程（並發與生命週期）與前端兩個模組，後端的狀態與事件是前端的基礎，錯了返工貴；
> task 超過 8 個。

每個 task 完成前跑該範圍的品質 gate（`AGENTS.md`「桌面小工具宿主」的品質 gate）；Rust task 先載入 `rust-engineer`。

## 1. 後端：活動狀態與通知

- [x] 1.1 在 `host/src/fetch/` 定義 `FetchActivity`（`running`／`enabled`／`last`）與其序列化形狀；`SchedEnv` 加
  `activity` 回報方法（預設空實作）。驗證：單元測試斷言序列化後的 JSON 欄位名與三種 `last` 字串。
- [x] 1.2 `tick`／`run_loop` 在閘門改變、一輪開始前、一輪結束後回報活動狀態；`LoopState` 記住上次送出的值，醒來時
  不一致就補送。驗證：以假環境測試下列路徑各自送出的序列——正常一輪（true → false＋`last: ok`）、全部來源失敗
  （`last: failed`）、輪被停止要求中止、閘門在輪中關閉（`enabled: false`）、輪 panic、行程啟動時由排程紀錄初始化 `last`。
- [x] 1.3 `HostEnv::activity`：寫 managed state、`running` 由真變假時先同步跑一次資料輪詢（含桌布協調迴圈的
  `DataChanged` 通知）再廣播 `fetch-activity`。先讀 `data.rs` 確認 `poll_all_changed` 的鎖範圍並把結論寫進程式註解。
  驗證：測試斷言「輪詢先於結束廣播」的呼叫順序；兩條執行緒同時輪詢不會推出較舊世代的快照。
- [x] 1.4 指令 `get_fetch_activity`（小工具視窗與設定視窗可呼叫，其餘 label 回錯誤）並在 `main.rs` 兩處 handler 清單註冊；
  事件名 `fetch-activity` 加進 `self_test_ipc.rs` 的 `BARE_PROBE_EVENTS`。驗證：label 判斷的單元測試；
  `cargo test --features self-test-ipc` 的事件覆蓋守門測試通過。

## 2. 後端：手動更新

- [x] 2.1 `sched::Trigger` 加 `Manual`（記錄字串 `manual`）；喚醒訊號加「手動要求」旗標，`wait` 在停止或手動要求時提早
  返回。驗證：單元測試——手動要求喚醒另一條執行緒中的 `wait`、回傳值仍只表示停止、`trigger_names_are_the_log_strings`
  補上新字串。
- [x] 2.2 `tick` 取走手動旗標：閘門啟用時直接開始 `Manual` 一輪（不經時點與 60 分鐘下限）、照常寫排程紀錄；閘門未啟用時
  丟棄；一輪結束時清旗標。驗證：假環境測試對應規格情境——排程剛抓完 3 分鐘後手動立刻開始；手動後 20 分鐘的時點延後到
  下限屆滿；抓取進行中送來的要求不會造成緊接的第二輪；閘門關閉時不抓；手動輪失敗後走既有重試退避。
- [x] 2.3 指令 `request_fetch_now`（只收設定視窗）：回 `started`／`already_running`／`disabled`，沒有排程把手回錯誤；
  註冊進 handler 清單。驗證：純函式層的三種結果與 label 拒絕各有單元測試。

## 3. 前端：小工具

- [x] 3.1 `bridge.js` 加 `getFetchActivity()` 與 `fetch-activity` 的 `listen` 支援（無 Tauri 環境下的測試替身一併補上）；
  `registry.js` 三個小工具加 `needsData`。驗證：`node host/tests/widget-init-race.test.mjs` 等既有外殼測試通過；
  registry 與 Rust 清單的一致性測試通過。
- [x] 3.2 `common.js` 加純函式：由（快照狀態、`needsData`、活動狀態）算出 `{ mode: "content" | "loading" | "empty", text }`
  與無資料文字。驗證：新增 `host/tests/fetch-activity-view.test.mjs`，涵蓋首次安裝未開始、進行中、抓取關閉、上一輪失敗、
  已有資料五種組合，以及「未失敗時文字不含連線失敗」。
- [x] 3.3 `widget.html`＋`widget.css`：訂閱活動狀態（先 listen 再查詢）、角落圖示、整塊載入區塊（期間回報有內容、解除時
  重報）、`prefers-reduced-motion` 與暫停時停止動畫、編輯版面時不與 A−／A+ 重疊；把活動狀態放進 `ctx`。
  驗證：新增外殼測試（載入 → 收到資料後回到內容；載入 → 失敗後回報無內容；ok 狀態下只出現角落圖示且內容容器大小不變）；
  `node host/tests/compare/compare.mjs` 在無抓取進行時結果與改動前相同。
- [x] 3.5 結束通知與快照的送達順序（design D8）：外殼收到 `running` 由真變假、小工具需要資料而沒有有效快照時，維持載入並
  主動查詢快照，回覆後才離開載入；查詢活動狀態失敗當成未知、等事件校正。驗證：外殼測試——「結束事件先到、快照後到」
  期間不出現無資料提示；快照查詢回覆確定沒有資料時才顯示無資料文字；查詢逾時後照當下狀態顯示；活動狀態查詢被拒後
  收到事件能恢復。
- [x] 3.4 `macro.js`、`dynamic.js` 的無資料分支改用 3.2 的文字；`dynamic.js` 過期提示改寫。行情條無資料時依
  `widget-host-windows`「行情條無資料」維持隱藏、不畫文字（抓取中由外殼的整塊載入顯示）。
  驗證：`node host/tests/dynamic-footer-age.test.mjs` 更新斷言後通過；三個模組在 empty 快照下的文字各有測試。

## 4. 前端：設定視窗

- [x] 4.1 `settings-fetch.mjs` 加純函式：由（抓取狀態、活動狀態）算出按鈕文字、是否停用與狀態文字。
  驗證：`node host/tests/settings-fetch.test.mjs` 新增三種狀態與「更新中」前綴的案例。
- [x] 4.2 `settings.html`：抓取狀態區塊加按鈕、訂閱 `fetch-activity`、結束時重查抓取狀態、`request_fetch_now` 各種回覆的
  處理；資料目錄提示文字改寫。驗證：以 `self-test-ipc` 建置開設定視窗，CDP 讀按鈕狀態——按下後停用並顯示更新中、
  一輪結束後恢復且完成時間改變（證據存 `host/tools/evidence/`）。

## 5. 系統匣圖示：新版橘點

- [x] 5.1 `app_icon.rs` 加純函式：在 RGBA 右下角畫帶深色描邊的橘色圓點（直徑比例為常數）。驗證：七個內嵌尺寸各一個
  單元測試——圓心像素為橘色、描邊像素為深色、圓點範圍外的像素與輸入逐位元組相同、輸出長度不變。
- [x] 5.2 系統匣套用路徑帶上橘點與提示文字：`refresh_in_order` 讀 `updater` 的待安裝版本；去重紀錄加入標記狀態；
  有待安裝版本時提示文字為「財經日曆（有新版 vX.Y.Z）」，否則「財經日曆」；設定視窗圖示不變。驗證：單元測試——
  同一圖示組與尺寸下標記狀態改變不被去重略過、提示文字兩種狀態、設定視窗的選擇不受標記影響。
- [x] 5.3 `updater.rs` 的 `notify_ready` 與 `refresh_menu` 呼叫 `app_icon::request_refresh`。驗證：原始碼守門測試
  （沿用 `app_icon.rs` 既有的呼叫點檢查寫法）並做變異驗證；`--features update-e2e` 的 clippy 與測試通過。
- [x] 5.4 實機：以 `update-e2e` 建置與本機端點讓宿主找到新版（不走 `--autostart`，避免自動安裝），確認橘點與提示文字出現、
  切換主題後仍在；100% 與 150% 縮放各截一張通知區域。開始前確認工作階段未鎖定、正式版宿主已由使用者從系統匣結束。
  證據存 `host/tools/evidence/`。
  - 2026-11-15 已驗：150% 下橘點與提示文字出現（`fetch-progress-5.4-tray-150*.png`，圖示 24px）。
  - 未驗：100% 截圖（要改主螢幕縮放）、切換主題後仍在（會接管使用者的桌布）。使用者 2026-11-15 決定接受現況、不補。

## 6. 整合驗收與文件

- [x] 6.1 實機（隔離資料夾、`data_fetch` 設 `on`）：空資料目錄啟動，截圖三個時點——抓取中（三個小工具整塊載入、時鐘與
  固定事件角落圖示）、抓完（全部有內容、圖示消失）、再按一次立即更新（角落圖示出現後消失）。開始前確認工作階段未鎖定、
  正式版宿主已由使用者從系統匣結束。證據存 `host/tools/evidence/`。
- [x] 6.2 斷網驗收：空資料目錄、網路不通，確認載入動畫在該輪結束後換成「尚無資料…產生 log.txt」而不是一直轉；
  `data_fetch` 為 `off` 時顯示抓取已關閉且設定視窗按鈕停用。
  （2026-11-15：使用者當下不能斷網，改以防火牆規則只擋測試版執行檔連外來模擬，連線錯誤是 10013 而非逾時。）
- [x] 6.3 更新 `AGENTS.md`（核心↔頁面的事件清單、系統匣與設定視窗的說明、新版橘點、Backlog 移除已完成項）與 `host/README.md`
  「資料抓取」；`npx markdownlint-cli2` 0 error。
- [x] 6.4 全套品質 gate（三種 feature 組合的 clippy 與測試、`cargo fmt --check`、前端 node 測試）並把輸出附在回報；
  diff 審查（Codex，額度用盡時由 opus subagent 代行）通過。
