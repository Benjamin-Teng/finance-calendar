# Tasks

> 執行路徑：SDD｜理由：14 個來源加上組裝、排程、宿主整合，跨 Python oracle、Rust 抓取模組與宿主啟動流程；含金融數值
> 捨入、時區、昨收序列等高風險面，需要逐 task 審查。
>
> 審查路由：逐 task 審查與最終全分支審查一律由 Opus subagent 執行，不用 Codex（使用者 2026-10-02 指示，優先於全域
> CLAUDE.md 的 Codex 規定）。
>
> 行為依據：`behavior-inventory.md`（B-*／C-*／U-*／K-* 編號）。每個來源 task 的驗收＝該來源的條目全數有對應測試，
> 且 Python oracle 對照通過。Python 一律 `uv run --no-project`，不裝套件。
>
> 依賴：`reqwest`（新增）——使用者 2026-10-05 授權本線依研究結論自行決定路線（見 design.md D2）。
>
> 歸檔順序：desktop-widget-host → dynamic-wallpaper → 本 change（MODIFIED delta 需要前兩者的基準規格）。

## 1. Python oracle 與樣本

- [x] 1.1 `tests/fetch_oracle/`（design.md D4）：共用的「隔離執行」包裝（腳本複製到暫存資料夾、`old.json` 當舊檔、
  `lively_wallpaper_dirs` 回空、`sys.argv` 只留腳本、`datetime` 凍結台北與本機兩種 `now`、`wait_for_network` 立即成功）；
  `record.py` 實跑一輪，把每個請求（方法、網址、表單）與回應（狀態碼、位元組；HTTP 錯誤也錄）寫進
  `host/tests/fixtures/fetch/recorded-<YYYYMMDD>/`（`manifest.json`＋回應檔＋錄製時間）；實跑一次產出第一組錄製樣本並提交。
  驗收：執行前後 repo 工作區（`git status`）與使用者的 Lively 夾都沒有出現新的 `tw_events.*`
- [x] 1.2 `tests/fetch_oracle/expect.py <情境目錄>`：以 manifest 回放（查無對應＝丟 `URLError`），輸出情境目錄的
  `expected.json`；同一情境跑兩次輸出相同。`uv run --no-project python -m unittest` 加測試確認錄製情境可重現、回放時
  沒有任何真實網路請求
- [x] 1.3 邊界情境：從錄製樣本複製並修改，依 behavior-inventory K 區與各來源條目構造至少下列情境，每個情境附
  `README.md` 說明目的與對應條目：全部來源失敗＋有舊檔、全部失敗＋無舊檔、上櫃處置失敗、TPEx 空白樣板列、二度處置、
  處置期間雙格式、法說會區間日期、MOPS 一個月失敗走備援、前百大為空走備援、前百大同日快取命中、下週總經 404、
  除權息主端點失敗走 openapi、行情單檔失敗沿用、昨收序列（盤中即時報價＋當日 K 為 null）、SOFR 主端點失敗走備援、
  休市日說明列、盤中走勢 12:00 與 15:00、日 K 倒退、券資比日期不一致。逐一以 `expect.py` 產出 `expected.json`。
  刻意改良情境（舊檔各鍵型別錯誤、總經單列 `forecast` 非字串、某來源解析 panic）期望檔手寫，`README.md` 標明

## 2. Rust 骨架

- [x] 2.1 依賴：`reqwest 0.13`（`default-features = false`，`native-tls`、`system-proxy`）；`regex`、`time` 明列所需
  feature；確認 `cargo tree` 中沒有 rustls、aws-lc、ring
- [x] 2.2 `host/src/fetch/`：`Fetch` 介面（方法、網址、表單 → 狀態碼＋位元組）、reqwest 實作（UA、30 秒逾時、跟隨轉址、
  系統代理、嚴格憑證驗證、同主機請求間隔 ≥2 秒）、`FixtureFetch`（讀 1.1 的 manifest）、時間注入、`output.rs`（輸出結構
  依 B-OUT 鍵序、UTF-8 無 BOM、`.tmp` 再改名）、`dates.rs`（`roc_to_date` 兩條 regex、台北 +8、與 Python 一致的
  `round2`、`%g`）、錯誤訊息正規化（design.md D4 的前綴清單，供 4.2 使用），各附單元測試（`round2` 以 0.125、2.675、
  1.005 等 Python 實際輸出為期望值）

## 3. 來源移植（每項：條目全有測試＋該來源相關情境的 oracle 對照；每個來源呼叫各自隔離 panic，design.md D7）

- [x] 3.1 總經 ForexFactory（B-FF、C-6 字典 94 條逐字、後綴取代；K-14 單列略過為刻意改良）、SOFR（B-SOFR，含備援）、
  休市日曆（B-HOL、K-9）
- [x] 3.2 除權息（B-DIV，主端點＋openapi 備援、`fmt_cash`）、股東會（B-MTG）、市值前百大（B-TOP、K-3、K-17）
- [x] 3.3 法說會 MOPS（B-CONF，兩個月、區間日期、季別判定 `quarter_of` 的 Unicode 語意 K-5）＋重大訊息備援（B-NEWS、
  B-CONF-10）
- [x] 3.4 處置股上市＋上櫃（B-PUN、K-8）、行情 Yahoo（B-QT，昨收自序列取 K-2、逐檔沿用）
- [x] 3.5 動態桌布三鍵：盤中走勢（B-ID）、日 K（B-DK）、券資比（B-MG）、共用護欄（B-GRD）；移植
  `tests/test_wallpaper_data.py` 對應的全部案例

## 4. 組裝與等價

- [x] 4.1 一輪的組裝（B-FLOW-3～11、B-EVT）：讀舊檔（型別不對的鍵視為不存在）、順序呼叫、各粒度沿用、窗口／去重／
  排序、counts、`errors`／`wallpaper_errors` 字串（B-FLOW-9 逐字）、`fetched`（B-FLOW-7）、`updated` 本機時間、原子寫檔、
  每個來源隔離 panic、整輪外層防線（design.md D7）
- [x] 4.2 等價測試：對 `host/tests/fixtures/fetch/` 下每個情境以 `FixtureFetch`＋凍結時間跑一輪，與 `expected.json` 以
  JSON 值比對（錯誤訊息先正規化）；刻意改良情境另列。全部通過才算完成

## 5. 宿主整合

- [x] 5.1 `--fetch-once <目錄>`（design.md D10）：在單一執行個體與 Tauri 建立前處理；不看 `data_fetch`、不讀寫排程紀錄；
  暫存檔名含行程 ID；結束碼依 spec；有整合測試（以測試專用注入點指向樣本目錄——只在 `cfg(test)` 或測試 feature 下存在）
- [x] 5.2 排程（design.md D5）：每 60 秒以系統時鐘判斷、台北時點、「到點」與「錯過」的區分、隨機延遲、啟動與睡眠恢復
  補抓、60 分鐘下限、全部失敗後的重試與加倍、資料檔不存在時的快速首抓、時鐘倒退與排程紀錄損毀、等網路（B-FLOW-2）、
  停止介面（供系統匣結束與 installer-auto-update 使用）、排程紀錄 `fetch-state.json`；排程判斷寫成純函式，以注入的時鐘
  測試 spec 各 Scenario（隔夜開機、睡眠跨過時點、未錯過時點、喚醒緊接時點、首次安裝、首次安裝時網路不通）
- [x] 5.3 抓取開關（design.md D10）：`settings.rs` 加 `data_fetch`（`auto`／`on`／`off`，不改 `SETTINGS_VERSION`、
  經設定更新指令變更時即時生效、手改設定檔於下次啟動生效、不認得的值視為 `auto`）；`desktop.rs` 加隔離偵測（KnownFolder 與環境變數比較），單元測試涵蓋大小寫與結尾
  斜線；以 grep 列出所有以環境變數隔離宿主的 `host/tools/*.ps1`，實跑 `verify-datadir-empty.ps1` 與一支放固定樣本的腳本，
  確認資料目錄內容與修改時間不變、記錄檔寫明「隔離環境，不抓取」。`rm-restart-test.ps1` 寫入 `"data_fetch": "off"`
  由收尾線在合併後處理（RmRestart 重啟的行程拿到真實環境、會判為非隔離），報告列為對照項
  （收尾線已完成：613475c，情境 A／B 放入真實路徑的設定檔皆寫 `"data_fetch": "off"`，
  靜態測試 `host/tools/tests/RmRestartDataFetch.Tests.ps1`）
- [x] 5.4 日誌：每輪開始、各來源成敗與沿用、寫檔結果寫入宿主記錄（沿用 `logging.rs`），摘要格式參考 B-FLOW-18
- [x] 5.5 資料來源與抓取狀態（design.md D12）：`host/ui/settings.html` 底部加資料來源、授權條款連結與抓取狀態（上一輪
  完成時間、來源失敗、「未抓取」與原因）；查詢指令單元測試涵蓋三種狀態；headless 截圖確認可見

## 6. 實網與實機驗收

- [x] 6.1 實網對照：同一時段先以 1.1 的隔離包裝跑一輪真實 Python（輸出到暫存目錄 A、不碰 repo 與 Lively 夾）、再跑
  `fc-host.exe --fetch-once <暫存目錄B>`，兩邊都從空目錄開始；比對兩份的鍵集合、各 `counts`、`errors` 筆數、`quotes`
  名稱與順序、`events` 集合；差異逐條解釋（來源在兩次請求間變動者除外）並附在報告
- [x] 6.2 實機（隔離宿主、不碰使用者桌布；設定檔寫 `data_fetch: "on"` 以覆寫隔離偵測）：空資料目錄啟動，記錄檔顯示
  15 秒內開始抓取（不計等網路）、該輪完成後寫出 `tw_events.json` 與排程紀錄、小工具顯示資料；改回不寫 `data_fetch`
  （`auto`）重啟，30 分鐘內資料目錄未被改寫、記錄檔寫明隔離環境
- [x] 6.3 品質 gate：`host/` 內 `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`（另跑
  `--features self-test-ipc`）、`cargo test`；`uv run --no-project python -m unittest discover -s tests`；改過的 `.md`
  跑 `npx markdownlint-cli2`
- [x] 6.4 文件：AGENTS.md「現況」資料流與「桌面小工具宿主」資料流改以宿主為準（Python 版標註只服務凍結的 Lively）；
  `host/README.md` 補 `--fetch-once` 與 `data_fetch`（含隔離偵測）；dynamic-wallpaper tasks.md 2.5 註明由本 change 取代；
  `openspec validate data-layer-rust --strict` 通過
