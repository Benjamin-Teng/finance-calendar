# quote-prev-close-intraday-extra-point

盤中 Yahoo 在「當日 K」之外另附一筆即時報價，同一交易日佔兩個位置：昨收要依 `gmtoffset` 換算的當地日期歸戶，取最後一個「不同於當前交易日」的收盤，而不是倒數第二筆，更不是 `chartPreviousClose`（那是 range 起點之前的收盤）。

- 對應條目：B-QT-3、B-QT-4、K-2
- 構造方式：
  - `USDTWD=X`：`gmtoffset` 3600，5 根日 K（當地午夜）收盤 31.1／31.2／31.3／31.4／null（當日 K 還沒有收盤）＋一筆即時報價 31.5（同一當地日期的 08:58）；`chartPreviousClose` 刻意設成 99.0。
  - `CL=F`：`gmtoffset` -14400，日 K 收盤 70.1／70.2／70.3／70.4／70.45（當日 K 已有部分收盤值）＋即時報價 70.5；`chartPreviousClose` 刻意設成 12.34。70.4 與 70.45 的差別用來擋「取倒數第二筆有效收盤」的寫法。
  - `^N225`（專擋「不吃 `gmtoffset`、以 UTC 日期歸戶」的實作）：`gmtoffset` 32400，日 K 時間戳在 JST 00:00（＝前一個 UTC 日 15:00），收盤 99.0／99.5／100.0／100.5（最後一根是當日 K、已有收盤值）＋一筆 JST 10:00 的即時報價 101.0（UTC 時間已跨到下一日）；`chartPreviousClose` 刻意設成 50.0。以 UTC 歸日時當日 K 與即時報價分屬兩個日期，會把當日 K 的 100.5 當成昨收；依當地日期歸戶才得 100.0。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - 「USD/TWD」`price` 31.5、`prev` 31.4（前一個當地日的收盤，不是 99.0）。
  - 「WTI 原油」`price` 70.5、`prev` 70.4（不是當日 K 的 70.45，也不是 12.34）。
  - 「日經225」`price` 101.0、`prev` 100.0（不是當日 K 的 100.5，也不是 50.0）——把 `gmtoffset` 改成 0 這筆就會變 100.5，測試因此失敗。
  - 三檔的 `chg_pct` 為 `(price/prev-1)*100` 不捨入；其餘 10 檔不受影響。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。
- `expected.json` 由 `tests/fetch_oracle/expect.py` 對本目錄回放 Python 版產生，未手改。
