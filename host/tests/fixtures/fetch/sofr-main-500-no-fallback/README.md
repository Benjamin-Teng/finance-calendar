# sofr-main-500-no-fallback

SOFR 主端點（`last/2`）HTTP 失敗：Python 直接視為失敗、沿用舊檔的 SOFR，**不會**打 `last/5` 備援（備援只處理「成功但筆數不足」）。Rust 版要照此行為，不可因主端點失敗就去打備援。

- 對應條目：B-SOFR-1、B-SOFR-2、B-SOFR-6、B-FLOW-7
- 構造方式：
  - `last/2` 覆寫成 HTTP 500；`last/5` 覆寫成有效的 200（與 `sofr-last5-fallback` 同內容）當絆線。
  - `old.json` 的 SOFR 3M 改成可辨識的舊值（price 1.11、prev 2.22）。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - SOFR 3M 等於舊檔那一筆（price 1.11）——若打了備援會是 3.7。
  - `errors` 恰一筆：「行情來源失敗（SOFR 3M／NY Fed）：HTTP Error 500: Internal Server Error」；`counts.quotes` 為 13，SOFR 仍在最後。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。
- `expected.json` 由 `tests/fetch_oracle/expect.py` 對本目錄回放 Python 版產生，未手改。
