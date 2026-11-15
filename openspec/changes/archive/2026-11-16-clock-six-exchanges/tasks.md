# Tasks：clock-six-exchanges

> 執行路徑：直接 `opsx:apply`｜理由：只動時鐘一個小工具的前端（一個新的純函式模組、`clock.js`、`widget.css`）與對照／量測
> 工具，task 線性且少，無 Rust、IPC 或設定結構變更。品質 gate 與 Codex review 照跑。

## 1. 開收盤判斷（`host/ui/exchange-clock.mjs`）

- [x] 1.1 依 design.md D1 先寫 `host/tests/exchange-clock.test.mjs`（紅燈），再實作 `exchangeRows(config, nowMs)`。測試直接讀
  內建 `wallpaper-config.default.json`，涵蓋 spec 的六個 Scenario，另加：六列順序與簡寫、週末全灰、開盤與收盤的邊界分鐘
  （台北 08:59 灰、09:00 白、13:29 白、13:30 灰）、`config` 為 `null` 時全灰且時間日期正確、設定檔缺某個市場時該列仍有時間。
  驗證：`node --test host/tests/exchange-clock.test.mjs` 全過。
- [x] 1.2 變異驗證：把 `open` 的判斷改錯（例如把 `night` 也算開盤、拿掉 `auction`）各一次，確認測試會紅，再改回。
  驗證：兩次變異都有測試失敗，輸出記在回報裡。

## 2. 時鐘小工具

- [x] 2.1 改寫 `host/ui/widgets/clock.js`（design.md D2）與 `host/ui/widget.css` 的時鐘區塊（D3），移除 `#clockTime`、
  `#clockDate`、`#weekRange`。驗證：headless Edge 開 `widget.html?w=clock`（fixture 模式）截圖，六列內容正確、開盤列為白色。
- [x] 2.2 改 `host/tests/compare/measure-clock-natural-width.mjs` 量新版面並實跑。驗證：輸出的整扇視窗自然尺寸（含 gap 與
  4 px 餘裕）不超過 212×160；超過就回頭調字級。
- [x] 2.3 更新綁舊節點 id 的前端測試；時鐘的顯示文字存成基準檔（change `retire-lively` 的 `golden/clock.txt`）。驗證：
  `node host/tests/compare/compare.mjs --widget clock` 回報 PASS，`host/tests/*.test.mjs` 全過。

## 3. 文件與收尾

- [x] 3.1 更新 `AGENTS.md`「現況」的小工具說明與 `host/tests/compare/README.md` 中提到時鐘對照的段落。驗證：
  `npx markdownlint-cli2` 對改過的 `.md` 0 error。
- [x] 3.2 品質 gate：`host/` 內 `cargo fmt --check`、`cargo clippy --all-targets`、`cargo test`（前端有被 Rust 測試讀到的檔案時
  才會受影響）；`openspec validate clock-six-exchanges --strict`。驗證：全部通過，輸出在回報裡。
- [x] 3.3 Codex adversarial-review（額度用盡，依規定改由 opus subagent）審這個 change 的 diff，findings 實測重現後才採信。驗證：無未處理的常規路徑問題。
- [x] 3.4 重建正式版執行檔（`cargo clean --release -p fc-host && cargo build --release`）交使用者實機驗收。
