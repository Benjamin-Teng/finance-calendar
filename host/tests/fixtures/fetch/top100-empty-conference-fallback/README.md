# top100-empty-conference-fallback

市值前百大算不出任何一家（來源欄位改名）且沒有舊名單：前百大記錯、名單為空，法說會因此直接走重大訊息備援（不打 MOPS）。

- 對應條目：B-TOP-5、B-TOP-6、B-TOP-7、B-CONF-1、B-CONF-10、B-NEWS、D-4
- 構造方式：
  - `t187ap03_L` 覆寫成兩筆手工列，股數欄位改名為「已發行股數」（Python 讀的是「已發行普通股數或TDR原股發行股數」），算不出任何市值；`STOCK_DAY_ALL` 維持錄製回應（券資比也要用它）。不放 `old.json`。
  - `t187ap04_L` 覆寫成 2 筆手工重大訊息（`2330` 日期取自說明、`2408` 日期取自事實發生日）。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - `errors` 恰為兩筆、依序：「市值前百大來源失敗：市值一家都算不出來（來源欄位可能改名）」、「法說會：無市值前百大名單可篩，本輪改用備援來源」。
  - `top100` 為空陣列、`top100_date` 為 `null`。
  - `counts.earnings` 為 0；`events` 的法說會恰為備援產出的 `2330`（2026-10-15）與 `2408`（2026-10-12、「法說會（線上）」）。
  - 券資比仍照常算出（`margin` 存在，證明 `STOCK_DAY_ALL` 未被動到）。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。
- `expected.json` 由 `tests/fetch_oracle/expect.py` 對本目錄回放 Python 版產生，未手改。
