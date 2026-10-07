# all-sources-fail-with-old

每個來源都連線失敗（含備援），但舊檔完整：各鍵都沿用舊檔，`fetched` 保持舊值、`updated` 照常刷新。

- 對應條目：B-FLOW-6、B-FLOW-7、B-FLOW-9、B-FLOW-10、B-FF-11、B-PUN-7、B-QT-7、B-SOFR-6、B-HOL-2、B-TOP-6、B-GRD-1、K-12、K-15
- 構造方式：
  - 所有請求（錄製樣本的 28 個，加上錄製時沒走到的除權息 openapi、重大訊息、SOFR last/5）覆寫成連線錯誤 `URLError: <urlopen error [Errno 11001] getaddrinfo failed>`。
  - `old.json`＝`recorded-20261005/expected.json`，只把 `updated`／`fetched`／`top100_date` 改成前一天（`2026-10-04 12:00`／`2026-10-04 06:00`／`2026-10-04`），讓前百大不走同日快取、確實打到失敗的來源。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - `fetched` 等於舊檔的 `2026-10-04 06:00`（沒有任何來源抓到新資料），`updated` 是凍結時間 `2026-10-05 01:12`。
  - `macro`、`events`、`punish`、`quotes`（13 筆含 SOFR）、`top100`、`holidays`、`twii_intraday`、`twii_daily`、`margin` 都與舊檔相同；`top100_date` 保留 `2026-10-04`。
  - `errors` 共 22 筆（本週總經、除權息、股東會、前百大、MOPS、重大訊息、處置股上市／上櫃、12 檔行情、SOFR、休市日曆各一）；`wallpaper_errors` 3 筆（盤中走勢、日 K、券資比）。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。
- `expected.json` 由 `tests/fetch_oracle/expect.py` 對本目錄回放 Python 版產生，未手改。
