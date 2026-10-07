# punish-tpex-blank-template

TPEx 沒有處置資料時回「單筆全空白樣板列」而不是空陣列：被代號過濾擋掉，不算錯誤，也不會觸發沿用舊檔。

- 對應條目：B-PUN-2、B-PUN-4、B-PUN-6、B-PUN-7、K-8、K-12
- 構造方式：
  - `tpex_disposal_information` 覆寫成一筆所有欄位都是空字串的列（欄位名稱取自錄製樣本的 TPEx 列；空白樣板列的確切形狀依 AGENTS.md 的描述，過濾分支只看代號是否為空，與其餘欄位值無關）。
  - `old.json` 的 `punish` 帶有可辨識的上櫃舊列（同 `punish-tpex-fail`），用來證明「抓取成功但零筆」時不會沿用舊上櫃。
- 預期在 `expected.json` 看到的證據（`tests/test_fetch_oracle.py` 逐項斷言）：
  - `errors` 為空（空白樣板列不是錯誤）。
  - `punish` 沒有任何上櫃列，只有錄製的 8 筆上市；舊檔的 `3293`、`6488`、`2002` 都不在輸出。
- 回放底稿：`../recorded-20261005`（`scenario.json` 的 `base`），只覆寫上列請求；其餘請求照錄製回應。
- `expected.json` 由 `tests/fetch_oracle/expect.py` 對本目錄回放 Python 版產生，未手改。
