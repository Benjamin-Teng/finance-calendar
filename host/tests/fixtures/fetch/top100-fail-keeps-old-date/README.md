# top100-fail-keeps-old-date

前百大來源失敗：沿用舊名單，且 `top100_date` 必須保留舊值（不能改成今天，否則會被當成今天已算過而整天不再重試）。

- 對應條目：B-TOP-6、K-3、B-CONF-5
- 構造方式：
  - `old.json` 的 `top100` 換成可辨識的 4 檔（`2330`、`2308`、`2412`、`2317`），`top100_date` 為前一天 `2026-10-04`。
  - `t187ap03_L` 覆寫成 HTTP 500。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - `errors` 恰有一筆：「市值前百大來源失敗：HTTP Error 500: Internal Server Error」。
  - `top100` 為舊檔 4 檔、`top100_date` 仍是 `2026-10-04`。
  - MOPS 以舊名單篩選：財報只有 `2308`（10-05）與 `2330`（10-15），不再有 `3008`、`2408` 等。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。
- `expected.json` 由 `tests/fetch_oracle/expect.py` 對本目錄回放 Python 版產生，未手改。
