# sofr-last5-fallback

SOFR 的 `last/2` 回應成功但有效筆數不足 2 筆：改抓 `last/5`，過濾無效筆、依 `effectiveDate` 降冪取最新兩筆。注意：備援只在「主端點回應成功但筆數不足」時才觸發；主端點 HTTP 失敗時不會打備援（見 `sofr-main-500-no-fallback`）。

- 對應條目：B-SOFR-1～B-SOFR-5
- 構造方式：
  - `last/2` 覆寫成 200：兩筆，其中 `2026-10-01` 的 `average90day` 為 null（有效筆數只剩 1）。
  - `last/5` 覆寫成 200：5 筆、故意亂序，含一筆 `average90day` 為 null、一筆缺 `effectiveDate`（值 9.9，應被濾掉）。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - SOFR 3M 的 `price` 為 3.7（`2026-10-02`）、`prev` 為 3.6（`2026-09-30`，null 與缺日期的筆都被跳過）。
  - `t` 為 `2026-10-02` 當日 UTC 00:00 的 epoch 秒 1790899200；`errors` 為空；SOFR 在 `quotes` 最後一筆。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。
- `expected.json` 由 `tests/fetch_oracle/expect.py` 對本目錄回放 Python 版產生，未手改。
