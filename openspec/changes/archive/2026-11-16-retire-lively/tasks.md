# Tasks：retire-lively

> 執行路徑：直接 `opsx:apply`｜理由：刪檔、把一支測試工具改成基準檔比對、改文件與規格敘述，線性且不動產品邏輯。

## 1. 對照測試改為基準檔比對

- [x] 1.1 刪除前以舊的跨版本比對重跑所有情境。驗證：除 `dynamic-stale-3d` 的頁尾文案（v0.3.6 刻意改寫）外全部 PASS。
- [x] 1.2 新增 `scenarios.mjs`、改寫 `compare.mjs`、產生 `golden/`。驗證：`node host/tests/compare/compare.mjs` 全部 PASS；
  `--scenario clock --inject-diff` 回報 FAIL。
- [x] 1.3 `serve.mjs` 拿掉 `/lively/`，`evidence.mjs` 的命名與測試跟著改。驗證：`node host/tests/compare-evidence.test.mjs`
  通過，同目錄其他用到 `startServer` 的腳本抽跑仍可執行。

## 2. 刪除 Lively 版

- [x] 2.1 `git rm` 產品檔與以 Lively 為基準的測試（清單見 proposal.md）。驗證：`git grep` 找不到對已刪檔案的執行期引用。

## 3. 文件與規格

- [x] 3.1 `AGENTS.md`、`README.md`、`host/README.md`、`host/tests/compare/README.md`、`tests/fixtures/README.md`、
  `openspec/config.yaml`、程式註解。驗證：`npx markdownlint-cli2` 對改過的 `.md` 0 error。
- [x] 3.2 `clock-six-exchanges` 的 delta 改成只新增時鐘需求。驗證：兩個 change 都通過 `openspec validate --strict`。
- [x] 3.3 品質 gate：`host/` 內 `cargo fmt --check`、`cargo clippy --all-targets`、`cargo test`，前端 `host/tests/*.test.mjs`，
  Python `unittest`。驗證：全部通過。
