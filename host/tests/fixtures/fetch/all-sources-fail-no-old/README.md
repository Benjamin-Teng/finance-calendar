# all-sources-fail-no-old

每個來源都失敗且沒有舊檔：寫出全空的輸出，鍵的省略規則與 `fetched` 退回規則都被走到。

- 對應條目：B-FLOW-6、B-FLOW-7、B-OUT（鍵的省略條件）、B-CONF-1、B-TOP-6、U-1、K-11、K-15
- 構造方式：
  - 與 `all-sources-fail-with-old` 相同的 31 筆連線失敗覆寫，但不放 `old.json`。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - `macro`、`events`、`punish`、`quotes`、`top100` 皆為空陣列，`top100_date` 為 `null`，`counts` 全 0。
  - `holidays`、`twii_intraday`、`twii_daily`、`margin` 四個鍵不存在（B-OUT 的省略條件）。
  - `fetched` 等於 `updated`（`old.fetched or old.updated or now_str` 退到 `now_str`）。
  - `errors` 共 22 筆，其中前百大為空時的整句訊息「法說會：無市值前百大名單可篩，本輪改用備援來源」逐字出現；`wallpaper_errors` 3 筆。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。
- `expected.json` 由 `tests/fetch_oracle/expect.py` 對本目錄回放 Python 版產生，未手改。
