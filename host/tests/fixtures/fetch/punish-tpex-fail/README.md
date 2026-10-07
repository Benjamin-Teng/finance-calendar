# punish-tpex-fail

上櫃處置股（TPEx）回 500、上市（TWSE）正常：上櫃按 market 沿用舊檔、上市以新抓的為準，舊檔裡過時的上市列不會被帶進來。

- 對應條目：B-PUN-6、B-PUN-7、B-PUN-10、B-FLOW-6、B-FLOW-9
- 構造方式：
  - `tpex_disposal_information` 覆寫成 HTTP 500。
  - `old.json`＝`recorded-20261005/expected.json`（日期改前一天），`punish` 換成：錄製的 8 筆上市＋一筆已過期的上市舊列（`2002 中鋼（舊檔）`）＋兩筆可辨識的上櫃舊列（`3293 鈊象（舊檔）`、`6488 環球晶（舊檔）`）。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - `errors` 恰有一筆，且是「處置股來源失敗（上櫃）：HTTP Error 500: Internal Server Error」。
  - `punish` 的上櫃部分恰等於舊檔那兩筆；上市部分等於錄製的 8 筆；`2002` 不在輸出（上市是新抓的，不沿用舊上市列）。
  - `counts.punish` 為 10，輸出依代號升冪。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。
- `expected.json` 由 `tests/fetch_oracle/expect.py` 對本目錄回放 Python 版產生，未手改。
