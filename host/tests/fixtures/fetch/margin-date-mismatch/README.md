# margin-date-mismatch

券資比的彙總（MS）日期與個股明細（ALL）日期不一致：視為來源失敗，記到 `wallpaper_errors` 並沿用舊檔。

- 對應條目：B-MG-2、B-MG-4、B-MG-7、K-10
- 構造方式：
  - `MI_MARGN` 個股明細（`selectType=ALL&…&date=20261002`）覆寫成手工極小回應：`date` 為 `20261001`、一張只有 1 列的明細表（欄位名稱與錄製樣本相同，其餘值為構造值）；彙總（MS）維持錄製回應（日期 `20261002`）。
  - `old.json` 的 `margin` 換成可辨識的舊值（日期 `2026-09-30`、券資比 9.99）。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - `wallpaper_errors` 恰一筆：「券資比／融資維持率來源失敗：彙總日期 2026-10-02 與個股明細日期 2026-10-01 不一致」。
  - `margin` 等於舊檔那筆；`errors` 為空。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。
- `expected.json` 由 `tests/fetch_oracle/expect.py` 對本目錄回放 Python 版產生，未手改。
