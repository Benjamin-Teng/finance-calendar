# deliberate-old-wrong-types

舊檔裡多個鍵的型別不對（`events` 是物件、`punish` 是物件、`quotes` 是字串、`top100` 是整數、`top100_date` 是整數），同時對應的來源都失敗而必須沿用舊值。Python 會在 `_old_events_of` 的 `e.get` 丟 `AttributeError` 而整輪崩潰、不寫檔。

- 對應條目：D8-3、B-FLOW-6、B-FLOW-17
- 構造方式：
  - 除權息（主端點與 openapi）、股東會、重大訊息、處置股（上市與上櫃）、`2330.TW` 行情、`t187ap03_L` 全部覆寫成 HTTP 500。
  - `old.json` 為上述五個型別錯誤的鍵（加上字串型的 `updated`／`fetched`）。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - （原始輸入不會產生 `expected.json`：測試確認 Python 在原始輸入下崩潰。）
  - `expected.json` 的內容：型別錯誤的鍵一律視為不存在——`events`、`punish` 空、`quotes` 只有成功的檔（缺「台積電」）、`top100` 為空、`top100_date` 為 `null`；法說會因前百大為空走備援並記「無市值前百大名單可篩」。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。

## 刻意改良情境

這是 design.md D8-3 的**刻意改良**：上一份輸出中型別不對的鍵視為不存在。Python 在此輸入下崩潰（`AttributeError`／`TypeError`），沒有可當期望的輸出，所以 `scenario.json` 的 `expected_from` 指向「等效的乾淨輸入」`old.clean.json`（把型別錯誤的鍵全部刪掉的舊檔），`expected.json` 由它產生。**Rust 版必須在本目錄原始輸入（`old.json`）下產出同一份 `expected.json`。**
