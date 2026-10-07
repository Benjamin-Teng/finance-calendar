# mops-month-fail-news-fallback

MOPS 本月成功、下月 POST 回 500：整個 MOPS 來源視為失敗（不保留已抓到的本月），改用重大訊息備援；備援只產 `conference`，財報事件整批消失。

- 對應條目：B-CONF-9、B-CONF-10、B-NEWS-1～B-NEWS-5、K-7
- 構造方式：
  - MOPS 11 月的 POST 覆寫成 HTTP 500（10 月仍是錄製回應，含 4 筆財報與 1 筆法說會）。
  - `t187ap04_L` 覆寫成手工 7 筆：鍵名帶尾端空白的「主旨 」與不帶的「主旨」、日期取自說明內的「召開法人說明會之日期」、日期退回事實發生日、主旨含「法人說明會」但條款不是第 12 款、第 11 款的無關公告、條款第 12 款但文字沒有法人說明會、日期空白、非前百大代號（備援不篩前百大）、條款寫成「第 12 款」（有空白）。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - `errors` 恰有一筆：「法說會來源（MOPS）失敗：HTTP Error 500: Internal Server Error」。
  - `counts.earnings` 為 0（錄製 MOPS 的財報全消失），`counts.conference` 為備援產出的筆數；備援事件包含 `2330`（10-15）、`2408`（10-12、「法說會（線上）」）、`3008`（10-13、「法說會（線上）」）、`9958`（10-14，非前百大也收）。
  - 第 11 款的 `1101`、沒有法人說明會字樣的 `1216`、日期解析不出的 `2603` 都不在輸出。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。
- `expected.json` 由 `tests/fetch_oracle/expect.py` 對本目錄回放 Python 版產生，未手改。
