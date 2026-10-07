# top100-same-day-cache

舊檔的 `top100_date` 等於凍結的台北日期且名單非空：前百大不發請求、直接沿用，連 `t187ap03_L` 回 500 也不會被碰到。

- 對應條目：B-TOP-2、B-TOP-6、K-3、B-FLOW-7
- 構造方式：
  - `old.json`＝錄製輸出，`top100` 換成可辨識的 3 檔（`2330`、`2317`、`2454`），`top100_date` 設成凍結日 `2026-10-05`。
  - `t187ap03_L` 覆寫成 HTTP 500 當作絆線：若有發請求，輸出會出現前百大失敗訊息。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - `errors` 為空（沒有碰 `t187ap03_L`）。
  - `top100` 就是舊檔那 3 檔、`top100_date` 為 `2026-10-05`。
  - MOPS 的篩選吃這份名單：輸出的財報只有 `2330`（2026-10-15），`counts.earnings` 為 1、`counts.conference` 為 0。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。
- `expected.json` 由 `tests/fetch_oracle/expect.py` 對本目錄回放 Python 版產生，未手改。
