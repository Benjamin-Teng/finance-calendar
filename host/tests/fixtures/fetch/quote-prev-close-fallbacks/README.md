# quote-prev-close-fallbacks

昨收的退回規則：序列裡「找不到不同日」時回 `None` 而非倒數第二筆（退回 `chartPreviousClose`）、沒有 `timestamp` 時才用倒數第二筆有效收盤、序列算出 0.0 也視為缺值而退回、兩邊都缺就是錯誤。

- 對應條目：B-QT-3、B-QT-4、B-QT-5、K-2
- 構造方式：
  - `^TNX`：三個時間點都落在同一當地日、`chartPreviousClose` 5.0 → 序列算不出昨收，退回 5.0（不是序列倒數第二筆 4.2）。
  - `^GSPC`：沒有 `timestamp` 欄、收盤 100／101／102、`chartPreviousClose` 7.0 → 取倒數第二筆 101.0。
  - `^DJI`：兩個交易日、前一日收盤 0.0、`chartPreviousClose` 40.0 → 序列給 0.0（假值）→ 退回 40.0。
  - `^IXIC`：收盤全是 null 且沒有 `chartPreviousClose` → 昨收缺值錯誤、舊檔沒有 → 省略。
  - `^SOX`：`regularMarketPrice` 缺漏 → 錯誤、省略。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - 「US 10Y」`prev` 5.0、「S&P 500」`prev` 101.0、「道瓊」`prev` 40.0。
  - `errors` 恰兩筆：「行情來源失敗（NASDAQ／^IXIC）：前一交易日收盤缺值或為 0」、「行情來源失敗（費半／^SOX）：regularMarketPrice 缺值」；輸出沒有「NASDAQ」「費半」，`counts.quotes` 為 11。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。
- `expected.json` 由 `tests/fetch_oracle/expect.py` 對本目錄回放 Python 版產生，未手改。
