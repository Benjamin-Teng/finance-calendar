# quote-single-failure-keeps-old

Yahoo 單檔失敗：舊檔有同名項目就原樣沿用（保持在清單原位置），沒有就省略；兩種都要記錯誤，不影響其他檔。

- 對應條目：B-QT-7、B-QT-8、B-QT-10、K-12
- 構造方式：
  - `^N225`（日經225）與 `^KS11`（KOSPI）的 Yahoo 回應覆寫成 HTTP 500。
  - `old.json` 的 `quotes` 把「日經225」換成可辨識的舊項（price 1.0、prev 2.0、t 1），並刪掉「KOSPI」。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - `errors` 恰兩筆、依序：「行情來源失敗（日經225／^N225）：HTTP Error 500: Internal Server Error」、「行情來源失敗（KOSPI／^KS11）：HTTP Error 500: Internal Server Error」。
  - 「日經225」等於舊項且仍在原位置（第 6 項）；「KOSPI」不在輸出；`counts.quotes` 為 12，SOFR 3M 仍在最後。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。
- `expected.json` 由 `tests/fetch_oracle/expect.py` 對本目錄回放 Python 版產生，未手改。
