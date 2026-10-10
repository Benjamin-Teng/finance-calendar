# Tasks：fixed-events-holiday-shift

> 執行路徑：直接 `opsx:apply`｜理由：只動台股固定事件一個小工具的前端（`common.js`、`fixed.js`、`widget.css`）與對照／量測工具，task 線性且少，無跨模組介面或 IPC 變更；`min_width` 若需調整只改 `widgets.rs` 一個常數。品質 gate 與 Codex review 照跑。

## 1. 順延計算（`host/ui/common.js`）

- [x] 1.1 依 design.md D2 先寫 `host/tests/fixed-holiday-shift.test.mjs`（紅燈），再實作 `nextTradingDay` 與 `fixedOccurrences(today, holidays, monthsAhead)`，`targetDay` 改呼叫 `nextTradingDay`。測試涵蓋：
  - 交易日不動、`orig === date` 且 `note` 為 `null`
  - 週六／週日順延到週一
  - 2026 春節：2/18 順延到 2/23，2/12、2/13 視為休市
  - 2026-10-10（週六）月營收順延到 10/12、2026-11-14（週六）Q3 財報順延到 11/16，`note` 為推定說明
  - 台指期 `note` 為期交所規則說明
  - 季結算在休市日也不動，標籤為「那指・道瓊期貨季度結算」
  - 上個月月底事件順延進本月（`k = -1`，例如 2024-03-31 → 4/1）
  - 休市日清單不含該年度時只依週末
  - 30 天上限回傳原日期
  - 結果依順延後日期排序

  驗證：`node --test host/tests/fixed-holiday-shift.test.mjs` 與既有 `node --test "host/tests/*.test.mjs"` 全數通過。

## 2. 小工具（`host/ui/widgets/fixed.js`、`host/ui/widget.css`）

- [x] 2.1 依 design.md D3／D4 改 `fixed.js`：改用 `common.fixedOccurrences`；讀 `ctx.snapshot`、訂閱 `ctx.onData`，休市日簽章有變才 `fullRender()`；順延列（本週與下一次）帶 `.badge.shifted` 與 `title`；「今天」徽章加 `today` class，`updateTimeSensitiveState()` 改用 `.badge.today`；`widget.css` 加 `.badge.shifted` 樣式；更新檔頭註解（不再「不吃 ctx.snapshot」、與 Lively 的刻意差異）。驗證：在 1.1 的測試檔補 DOM 無關的斷言（`fixed.js` 原始碼含 `.badge.today` 選取器、不再以裸 `.badge` 判斷今天），並以 headless Edge 開 `widget.html?w=fixed`、固定今天為 2026-10-11 截圖，確認本週出現「10/12 (一) 9月營收公布截止」與「原 10/10」徽章。

## 3. 對照與版面量測

- [x] 3.1 依 design.md D5：`host/tests/compare/panels.mjs` 的 `fixed` 條目標為刻意分歧並寫明原因，`compare.mjs` 遇到時跳過並印出原因、不算失敗；`host/tests/compare/README.md` 同步。驗證：`node host/tests/compare/compare.mjs` 跑完、`fixed` 顯示為跳過、其餘面板結果與改動前相同；`compare-evidence.test.mjs` 等既有測試通過。
- [x] 3.2 以 `measure-list-collapse.mjs` 在含順延徽章的情境（今天 2026-10-11）重量台股固定事件最小框寬；若大於現值，更新 `host/src/widgets.rs` 的 `WIDGET_SPECS`、`host/tests/list-collapse.test.mjs` 與量測報告（新報告放本 change 目錄）。驗證：`node --test host/tests/list-collapse.test.mjs` 通過；若動到 `.rs`，`host/` 內 `cargo fmt --check`、`cargo clippy --all-targets`（含 `--features self-test-ipc`、`--features update-e2e`）、`cargo test`（含 `--features self-test-ipc`、`--features update-e2e`）全過。

## 4. 文件與收尾

- [x] 4.1 更新 `AGENTS.md`：「現況」補台股固定事件遇休市順延；Backlog 刪「台股固定事件遇假日順延標示」，新增「財報截止例外：Q2 金融業與第一上市（櫃）8/31、資本額 100 億以上年報 3/16」。驗證：改過的 `.md` 在 repo 根目錄跑 `npx markdownlint-cli2 <檔>` 0 error。
- [x] 4.2 實機確認：`cargo clean --release -p fc-host && cargo build --release` 後啟動宿主，台股固定事件小工具在一般與窄寬度下徽章不重疊、不被裁切，編輯版面放大字級後仍可讀；「今天」與順延徽章同列時都顯示。驗證：截圖存證（視覺驗收前先確認工作階段未鎖定）。
- [x] 4.3 Codex adversarial-review 整個 change 的 diff，findings 實測重現後處理；`openspec validate fixed-events-holiday-shift --strict` 通過。
