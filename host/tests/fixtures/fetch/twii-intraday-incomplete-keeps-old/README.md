# twii-intraday-incomplete-keeps-old

15:00 執行時 Yahoo 還沒追上收盤（來源只有 10/2 的部分序列）：不拿不完整序列冒充完整日、也不偷偷退回更早一天當新資料，沿用舊檔並把錯誤記到 `wallpaper_errors`。

- 對應條目：B-ID-3、B-ID-4、B-ID-6、B-GRD-2
- 構造方式：
  - `now_tpe` 設為 `2026-10-02T15:00:00+08:00`；5m 回應覆寫成 `tests/fixtures/yahoo_twii_5m_5d.json` 的位元組複本（真實錄製，10/2 只到 10:27，末筆為 Yahoo 另附的即時報價；說明見 `tests/fixtures/README.md`）。
  - `old.json` 的 `twii_intraday` 換成 `2026-10-01` 的 3 點手工序列。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - `wallpaper_errors` 含一筆以「加權盤中走勢來源失敗：2026-10-02 序列終點」開頭、含「太早（來源尚未追上收盤）」的訊息；`errors` 為空。
  - `twii_intraday` 等於舊檔那筆（`2026-10-01`、3 點）。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。
- `expected.json` 由 `tests/fetch_oracle/expect.py` 對本目錄回放 Python 版產生，未手改。
