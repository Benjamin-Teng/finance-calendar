# twii-date-regression

新抓的資料日期早於舊檔（Yahoo 偶發回較舊視窗）：日期不倒退，沿用舊檔。日 K 會記一筆 `wallpaper_errors`，盤中走勢則靜默沿用（兩者行為不同）。

- 對應條目：B-ID-6、B-DK-4、K-10
- 構造方式：
  - 不覆寫請求（錄製的 Yahoo 回應最後一個交易日是 2026-10-02）。
  - `old.json` 的 `twii_daily` 多一根 `2026-10-06`、`twii_intraday` 的日期設成 `2026-10-06`（都晚於新抓的 10-02）。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - `twii_daily` 等於舊檔（最後一根 `2026-10-06`）。
  - `wallpaper_errors` 恰一筆：「加權日K來源回傳較舊的資料（最後一根 2026-10-02 早於上次的 2026-10-06），沿用上次資料」。
  - `twii_intraday` 等於舊檔，且沒有任何對應的錯誤訊息；`errors` 為空。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。
- `expected.json` 由 `tests/fetch_oracle/expect.py` 對本目錄回放 Python 版產生，未手改。
