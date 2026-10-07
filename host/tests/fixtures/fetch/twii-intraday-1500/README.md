# twii-intraday-1500

交易日 15:00 執行：當日 13:30 已過，錄製的 10/2 序列完整（含 13:30 收盤試撮點），盤中走勢與日 K 都取當日 10/2。

- 對應條目：B-ID-2、B-ID-3、B-ID-5、B-DK-2、B-DATE-6
- 構造方式：
  - 不覆寫任何請求；`now_tpe` 設為 `2026-10-02T15:00:00+08:00`。與 `twii-intraday-1200` 用同一組回應，只差時間。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - `twii_intraday.date` 為 `2026-10-02`、最後一點是 13:30（落在 5 分鐘格線上的收盤點保留），共 55 點。
  - `twii_daily` 最後一根是 `2026-10-02`、收盤 48475.74；`wallpaper_errors` 為空；`updated` 為 `2026-10-02 15:00`。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。
- `expected.json` 由 `tests/fetch_oracle/expect.py` 對本目錄回放 Python 版產生，未手改。
