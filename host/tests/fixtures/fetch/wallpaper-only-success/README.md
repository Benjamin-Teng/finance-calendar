# wallpaper-only-success

只有動態桌布三鍵（加權盤中走勢、日 K、券資比）的來源成功，其餘來源全部連線失敗，舊檔只有 `updated`／`fetched`：組裝層的 `fetched` 不可被桌布鍵刷新，桌布鍵的值照常更新，桌布鍵不進 `errors`。

- 對應條目：B-FLOW-7、B-GRD-2、B-FLOW-9、K-10（對應 `tests/test_wallpaper_data.py` 的 `MainIntegrationTest.test_new_keys_do_not_refresh_fetched`）
- 構造方式：
  - 與 `all-sources-fail-with-old` 同樣把錄製樣本的請求覆寫成連線錯誤 `URLError: <urlopen error [Errno 11001] getaddrinfo failed>`，**但不覆寫**：`^TWII` 5 分 K、`^TWII` 日 K、`MI_MARGN`（MS 與 ALL）、`STOCK_DAY_ALL`（券資比的收盤價表；前百大因 `t187ap03_L` 失敗，不會因它而抓到新資料）。
  - `old.json` 只有 `updated`／`fetched`（都是 `2026-09-01 10:00`），沒有任何桌布鍵，所以輸出中的三個桌布鍵一定來自本輪新抓。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - `fetched` 維持舊值 `2026-09-01 10:00`、`updated` 是凍結時間 `2026-10-05 01:12`。
  - `twii_intraday`、`twii_daily`、`margin` 都在；`wallpaper_errors` 為空。
  - `errors` 共 22 筆，沒有任何一筆屬於桌布鍵（盤中走勢、日 K、券資比）；其餘鍵都是空（`macro`／`events`／`punish`／`quotes`／`top100` 為空、`top100_date` 為 `null`、沒有 `holidays`）。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。
- `expected.json` 由 `tests/fetch_oracle/expect.py` 對本目錄回放 Python 版產生，未手改。
