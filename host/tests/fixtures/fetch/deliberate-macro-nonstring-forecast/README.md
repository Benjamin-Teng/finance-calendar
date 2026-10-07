# deliberate-macro-nonstring-forecast

ForexFactory 某一列的 `forecast`（或 `previous`）是數字而非字串：Python 的 `(r.get("forecast") or "").strip()` 對數字丟 `AttributeError`，沒有任何 try 接住，整個 `main()` 崩潰、不寫檔。

- 對應條目：D8-2、B-FF-10、K-14
- 構造方式：
  - 本週總經覆寫成手工 6 列：`ISM Services PMI`（正常）、`CPI y/y`（`forecast` 為數字 1.5）、`Final Manufacturing PMI`（`forecast` 為 null，Python 視為空字串，正常）、`Tankan Large Manufacturers Index`（`previous` 為數字 13）、一筆 Low 重要性（被濾掉）、一筆正常的 `FOMC Member Speaks`、一筆 `forecast` 為數字 0 的 `Retail Sales m/m`（Python 的 `0 or ""` 得空字串、**不崩潰**，與 null 同屬「falsy 值視為空字串、保留該列」，不可略過）。
  - `ff_this.clean.json`：同一份移除兩個型別異常列後的版本，供 `expected_from` 使用。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - （原始輸入不會產生 `expected.json`：測試確認 Python 在原始輸入下以 `AttributeError` 崩潰。）
  - `expected.json` 的 `macro` 恰為 4 列：`ISM 非製造業PMI`（`forecast` 55.1）、`Final Manufacturing PMI`（`forecast` 為空字串）、`FOMC Member Speaks`、`Retail Sales m/m`（`forecast` 為空字串）；只有「非空且非字串」的兩個列被略過（null 與 0 保留），其他來源不受影響、`errors` 為空。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。

## 刻意改良情境

這是 design.md D8-2／K-14 的**刻意改良**：總經單列欄位型別異常只略過該列。Python 在原始輸入下整輪崩潰，沒有可當期望的輸出，所以 `expected_from.overrides` 把本週總經換成 `ff_this.clean.json`（移除兩個異常列）。**Rust 版必須在本目錄原始輸入（`ff_this.json`）下產出同一份 `expected.json`。**
