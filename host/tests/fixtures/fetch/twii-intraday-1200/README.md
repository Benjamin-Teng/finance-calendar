# twii-intraday-1200

交易日中午 12:00 執行：當日 13:30 還沒過，不算完整交易日——即使錄製的 Yahoo 回應已含 10/2 的完整序列，盤中走勢與日 K 也都只到前一個交易日 10/1。

- 對應條目：B-ID-3、B-ID-4、B-DK-2、B-DATE-2、B-DATE-6
- 構造方式：
  - 不覆寫任何請求；`now_tpe` 設為 `2026-10-02T12:00:00+08:00`（週五，本機時間視同台北）。回應用錄製樣本（其 5m 序列含 10/2 完整一日）。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - `twii_intraday.date` 為 `2026-10-01`、起點 09:00、終點 13:25（10/1 一個完整日 54 點）。
  - `twii_daily` 最後一根是 `2026-10-01`（10/2 當日那根被丟棄）；`wallpaper_errors` 為空；`updated` 為 `2026-10-02 12:00`。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。
- `expected.json` 由 `tests/fetch_oracle/expect.py` 對本目錄回放 Python 版產生，未手改。
