# macro-this-week-fail-keeps-old

本週總經檔失敗：不管下週檔有沒有抓到，整個 `macro` 鍵沿用舊檔（不混用下週資料），並記錯誤。平日下週檔 404 屬正常、不記錯誤——那條路徑由錄製樣本 `recorded-20261005` 本身涵蓋（其 `errors` 為空，下週檔是 404）。

- 對應條目：B-FF-2、B-FF-11、B-FLOW-6
- 構造方式：
  - 本週總經覆寫成 HTTP 500；下週總經覆寫成 200（手工 1 筆美元高重要性 `Core CPI m/m`）。
  - `old.json` 的 `macro` 沿用錄製的 6 筆，第一筆標題改成「舊檔沿用的總經事件」。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - `errors` 恰有一筆：「總經來源失敗（本週檔）：HTTP Error 500: Internal Server Error」。
  - `macro` 完全等於舊檔的 `macro`（6 筆，下週那筆 `Core CPI` 沒有混進來）。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。
- `expected.json` 由 `tests/fetch_oracle/expect.py` 對本目錄回放 Python 版產生，未手改。
