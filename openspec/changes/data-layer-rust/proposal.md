## Why

新宿主 v0.1.0 要以安裝檔發佈，但資料仍靠 `update_tw_events.py`：使用者得另裝 Python、由 `setup.bat` 以系統管理員建排程，
而且輸出只投遞到 Lively 的複製夾，宿主的 `data_dir` 沒有人寫入，這是目前上線的最大阻擋。使用者 2026-10-05 決定
v0.1.0 就要把資料層改成 Rust、放進宿主，並搭配自動更新（另一個 change `installer-auto-update`），讓日後資料來源改版時，
抓取邏輯的修正能隨程式更新快速送達。

## What Changes

- 宿主新增**資料抓取**：在宿主行程內常駐，依台北時間 06:00、12:00、15:00、18:00、00:00 定時抓取；啟動或從睡眠恢復時
  若錯過時點就補抓。抓取前先等網路就緒；各台機器加隨機延遲，兩輪之間至少隔 60 分鐘，避免對來源造成負擔。
- 抓取結果照舊寫成 `<data_dir>/tw_events.json`（先寫暫存檔再改名），**鍵名、型別、語意與 Python 版完全相同**；
  宿主既有的檔案輪詢、小工具與動態桌布都不用改。
- 移植 Python 版全部來源與規則：總經日曆、除權息、股東會、市值前百大、法說會（含備援）、處置股、行情、SOFR、休市日曆、
  加權盤中走勢、日 K、券資比，以及「各來源失敗時沿用舊資料」、`fetched` 只在真的抓到新資料時刷新等行為。
- 以**錄製的真實回應**當共同樣本：Python 版與 Rust 版吃同一份樣本，輸出必須一致才算移植完成；另補 Python 版從未有測試的
  來源（總經、除權息、MOPS、處置股、行情與昨收、SOFR）。
- 刻意與 Python 不同的改良：HTTPS 一律嚴格驗證憑證，不再退回「不驗證」；單一來源或單列的非預期錯誤只影響該來源、
  上一份輸出中型別不對的鍵視為不存在——Python 版這些情況會整輪崩潰、不寫檔。
- 設定新增 `data_fetch`（`auto`／`on`／`off`，預設 `auto`）。`auto` 在驗收腳本以暫存資料夾隔離宿主時自動視同關閉
  （偵測 `LOCALAPPDATA`／`APPDATA` 與系統登記的使用者資料夾不同），既有的三十多支驗收腳本不必修改，也不會被真實資料
  覆寫樣本。
- 設定視窗標示資料來源與政府資料開放授權（openapi 授權條款要求標示來源）。
- 新增命令列 `--fetch-once <目錄>`：抓一輪、寫到指定目錄後結束，供實網對照與除錯。
- 不再產生 `tw_events.js`，也不再鏡像到 Lively 複製夾；Lively 版維持凍結，`update_tw_events.py` 留在 repo，作為對照基準。

## Capabilities

### New Capabilities

- `market-data-fetch`：宿主內的資料抓取，涵蓋排程與補抓、網路等待與限流、輸出檔契約與原子寫入、各來源沿用舊資料的規則、
  `fetched`／`errors` 語意、抓取開關與隔離環境、單次抓取指令、資料來源標示，以及與 Python 版行為等價的驗收方式。

### Modified Capabilities

- `widget-data-feed`（desktop-widget-host）：「資料通道」原寫「宿主 MUST NOT 改寫任何來源檔」，改為「資料通道」不改寫，
  `tw_events.json` 可由宿主內建的資料抓取產生。
- `wallpaper-market-data`（dynamic-wallpaper）：「每日 15:00 更新」由 Windows 排程器改為宿主抓取排程的 15:00 時點。

這兩個能力的基準規格要等 desktop-widget-host、dynamic-wallpaper 歸檔後才存在，因此本 change 必須在它們之後歸檔。

## Impact

- 程式：新增 `host/src/fetch/`（各來源模組、HTTP 抽象、輸出與排程）；`host/src/main.rs` 啟動抓取任務並處理
  `--fetch-once`；`host/src/settings.rs` 加 `data_fetch`；`host/ui/settings.html` 加資料來源標示。`host/src/data.rs` 不改。
- 依賴：新增 `reqwest 0.13`（關閉預設功能，只開 `native-tls`，也就是 Windows SChannel），加上 `regex`、`time`。
  後兩者已在依賴樹中，執行檔只帶一套 TLS。
- 測試資料：`host/tests/fixtures/fetch/`（錄製回應＋邊界案例＋Python 產出的期望輸出）；`tests/` 新增 Python 端錄製與
  產生期望輸出的腳本。
- 文件：AGENTS.md「資料流」與「現況」改以宿主為準；dynamic-wallpaper 的 2.5（`setup.ps1` 加 15:00 排程）由本 change 的
  15:00 時點取代（見上方 Modified Capabilities）。
- 對外來源：每台安裝的機器各自連線抓取，限流與使用條款風險見 design.md。
