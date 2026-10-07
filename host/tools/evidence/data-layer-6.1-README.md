# data-layer-rust 6.1 實網對照（2026-10-05）

- Python：05:34 台北，`tests/fetch_oracle/record.py`（隔離包裝，輸出不碰 repo 與 Lively 夾）→ `data-layer-6.1-python.json`。
  28 個請求，唯一失敗是 ForexFactory 下週檔 404（平日正常）。
- Rust：05:39–05:40 台北（隔 5 分鐘避開 ForexFactory 限流），`fc-host.exe --fetch-once <暫存目錄>`（release 建置、
  `LOCALAPPDATA` 指向暫存以免寫入真實記錄）→ `data-layer-6.1-rust.json`，結束碼 0，耗時 41 秒。
- 比對（解析成 JSON 後）：鍵集合相同；`counts` 相同（除權息 51、股東會 6、法說會 1、財報 4、總經 6、行情 13、處置 18）；
  `errors`、`wallpaper_errors` 兩邊皆空；`macro`、`events`、`punish`、`quotes`（13 檔名稱、順序與數值）、`holidays`、
  `top100`／`top100_date`、`twii_intraday`、`twii_daily`、`margin` 全部逐值相同。唯一差異是 `updated`／`fetched`
  （各自的執行時間）。
- TLS：所有 `*.twse.com.tw`、MOPS、TPEx、Yahoo、NY Fed、ForexFactory 端點在 SChannel 嚴格驗證下皆成功（Rust 輸出 `errors` 為空）。
