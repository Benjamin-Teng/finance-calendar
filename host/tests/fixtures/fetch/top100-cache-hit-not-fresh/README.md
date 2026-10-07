# top100-cache-hit-not-fresh

前百大走同日快取不算「抓到新資料」：其他會刷新 `fetched` 的來源（總經、除權息、股東會、法說會、SOFR、休市日曆）全部失敗時，`fetched` 必須維持舊值。

- 對應條目：B-FLOW-7、B-TOP-6、K-3、B-GRD-2
- 構造方式：
  - `old.json` 同 `top100-same-day-cache`（同日快取，`fetched` 為 `2026-10-04 06:00`）。
  - 覆寫成 HTTP 500：本週總經、除權息主端點與 openapi、股東會、MOPS 10 月、重大訊息、SOFR、休市日曆、`t187ap03_L`（絆線）。處置股、Yahoo 行情與動態桌布三鍵仍成功（它們本來就不計入 `fetched`）。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - `fetched` 仍是 `2026-10-04 06:00`，`updated` 是 `2026-10-05 01:12`。
  - `errors` 恰 7 筆（總經、除權息、股東會、MOPS、重大訊息、SOFR、休市日曆），沒有前百大失敗；`top100` 為舊檔 3 檔。
  - `macro`、`holidays` 與舊檔相同，`events` 的除權息、股東會、財報／法說會全沿用舊檔。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。
- `expected.json` 由 `tests/fetch_oracle/expect.py` 對本目錄回放 Python 版產生，未手改。
