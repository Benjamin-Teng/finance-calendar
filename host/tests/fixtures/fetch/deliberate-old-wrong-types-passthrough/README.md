# deliberate-old-wrong-types-passthrough

舊檔的 `macro` 與 `holidays` 是字串，且本輪總經本週檔與休市日曆失敗而沿用：Python 不會崩潰，而是**把字串原樣寫進輸出**（`macro` 變成 `"oops"`、`counts.macro` 變成字串長度、`holidays` 變成 `"oops"`）——比崩潰更隱蔽。

- 對應條目：D8-3、B-FF-11、B-HOL-2、B-FLOW-6
- 構造方式：
  - 本週總經、休市日曆覆寫成 HTTP 500（下週總經維持錄製的 404）。
  - `old.json`：`macro`、`holidays` 為字串。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - （原始輸入不會產生 `expected.json`：測試確認 Python 在原始輸入下輸出 `macro == "oops"`。）
  - `expected.json` 的內容：`macro` 為空陣列、`counts.macro` 為 0、沒有 `holidays` 鍵。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。

## 刻意改良情境

這是 design.md D8-3 的**刻意改良**：型別不對的舊鍵視為不存在，而不是原樣照抄。`scenario.json` 的 `expected_from` 指向 `old.clean.json`（刪掉型別錯誤鍵的舊檔）。**Rust 版必須在本目錄原始輸入（`old.json`）下產出同一份 `expected.json`。**
