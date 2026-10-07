# margin-price-date-mismatch

融資彙總日期（10/2）與 `STOCK_DAY_ALL` 的收盤價日期（10/1）不同：兩者不得混用，沿用舊檔，且**不記錯誤**（晚間公布前屬正常）。

- 對應條目：B-MG-5、B-MG-7、K-17、K-10
- 構造方式：
  - `STOCK_DAY_ALL` 覆寫成手工 1 列、`Date` 為 `1151001`（它同時是前百大與券資比的價格來源，同一個請求鍵）；因此 `old.json` 設定 `top100_date` 為凍結日、`top100` 為 3 檔，讓前百大走同日快取、不受這份極小價格表影響。
  - `old.json` 的 `margin` 為日期 `2026-09-30` 的可辨識舊值（與新抓的 10/2 不同日，才會繼續往下走到價格日期比對）。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - `margin` 等於舊檔那筆；`wallpaper_errors` 為空、`errors` 為空。
  - `top100` 為舊檔 3 檔（證明前百大走了快取）。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。
- `expected.json` 由 `tests/fetch_oracle/expect.py` 對本目錄回放 Python 版產生，未手改。
