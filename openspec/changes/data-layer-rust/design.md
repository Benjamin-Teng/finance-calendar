## Context

動機見 proposal.md。現況：

- `update_tw_events.py`（1319 行、純 stdlib）由 Windows 排程器每 6 小時、每日 15:00、登入與喚醒時執行，產出
  `tw_events.json`／`.js`，並鏡像到 Lively 複製夾。它的行為只存在程式裡；已整理成可逐條引用的清單
  `behavior-inventory.md`（編號 B-*、C-*、U-*、K-*，本文以編號引用）。
- 宿主 `host/src/data.rs` 的 `JsonFileSource` 每 30 秒比對修改時間與大小，讀 `<data_dir>/tw_events.json`；
  `validate_tw_events` 拒收四個陣列全空的檔（U-1）。消費端讀的鍵見 U-10。
- 依賴樹（Windows 目標）沒有 reqwest、rustls、native-tls；已有 tokio（Tauri 帶入）、`regex`、`time`
  （研究：`.superpowers/research/data-layer-tech.md`）。
- 2026-10-01 TLS 探針：SChannel、rustls+aws-lc、rustls+ring 三種組態在嚴格驗證下全部能連上 8 個來源端點；
  Python 失敗是 OpenSSL 3.5 對 TWCA 根憑證缺 SKI 的檢查（`docs/SPEC-data-layer-rust.md`，git 忽略）。
- 驗收腳本（`host/tools/*.ps1` 約 37 支）以把 `LOCALAPPDATA`／`APPDATA` 指向暫存資料夾的方式隔離宿主；其中有的在
  資料目錄放固定樣本，有的刻意從空資料目錄開始驗「沒有資料」的行為，多支不寫設定檔。宿主的設定與資料路徑都取自這兩個
  環境變數（`settings.rs` `default_data_dir`／`default_settings_path`）。
- 自動更新歸另一個 change `installer-auto-update`；本 change 不依賴它就能完成與驗收。

## Goals / Non-Goals

**Goals:**

- 宿主不依賴 Python、排程器與系統管理員權限，就能自己取得資料。
- 輸出與 Python 版逐鍵等價，並有自動化對照測試保護，日後修改來源時可以放心回歸。
- 新增來源或改來源時，只需要動單一來源模組與它的樣本。

**Non-Goals:**

- 不改輸出格式、不新增資料來源、不改前端小工具與 `data.rs`。
- 不做「立即更新」選單、來源健康度介面、可遠端下發的抓取規則（自動更新已提供修正管道，見 D9）。
- 設定視窗只加資料來源標示；`data_fetch` 只在設定檔中，不加介面。
- 不改 Lively 版與 `update_tw_events.py` 的行為（它保留為對照基準）。

## Decisions

### D1. 抓取放在宿主行程內，仍以 `tw_events.json` 當界線

抓取模組寫檔，`data.rs` 照舊輪詢讀檔，兩邊只透過檔案溝通。

- 替代方案「抓取結果直接推進 `ChannelRegistry`」：少一次檔案往返，但要改 `data.rs` 與快照流程，也失去「必要時
  可退回由外部程式產檔」與 `--fetch-once` 可單獨驗證的好處。不採用。
- 替代方案「獨立的 `fc-fetch.exe`，由宿主定時啟動」：隔離當機，但多一個執行檔要打包、簽章、更新。抓取邏輯以
  `catch_unwind`＋錯誤攔截隔離即可（D7）。不採用。

### D2. HTTP 用 reqwest async＋native-tls（SChannel）

`reqwest 0.13`，`default-features = false`，features `native-tls`、`system-proxy`（POST 表單自行以
`application/x-www-form-urlencoded` 編碼——實作自寫編碼，未開 `form` feature）；排程跑在專用執行緒上，每輪自建 current-thread tokio runtime（`--fetch-once` 相同）。

- SChannel 不需任何 C 建置工具，ARM64 也不需要；使用 Windows 憑證庫，企業代理的自簽根憑證也能用；探針已驗證可連上
  所有來源。
- 自動更新外掛 `tauri-plugin-updater` 也以 `default-features = false`＋`native-tls` 引入（installer-auto-update），
  整個執行檔只帶一套 TLS。
- 替代方案 rustls+aws-lc（reqwest 預設）：ARM64 要 clang-cl；rustls+ring：同樣要 clang，且不吃 OS 憑證庫。不採用。
- 不保留 Python 的「憑證驗證失敗就改不驗證」退回（B-HTTP-4、K-16）：探針已證明不需要，而且它會對所有主機放寬驗證。
- 逾時：單一請求 30 秒（B-HTTP-2），無自動重試；UA 沿用 B-HTTP-1。

### D3. 以 `Fetch` 抽象隔離網路，測試只用樣本

所有來源只透過一個介面取資料：給定方法、網址與表單，回傳狀態碼與位元組。正式環境是 reqwest 實作；測試用
`FixtureFetch`，從樣本目錄的對照表（方法＋網址＋表單 → 檔案、狀態碼）回應，查不到就當成網路錯誤。「現在時間」
同樣以參數注入（台北日期、本機時間兩個用途，見 B-DATE-2、B-DATE-3）。

不引入 wiremock／httpmock：只需要回放，不需要模擬伺服器。

### D4. 行為等價以「Python 當 oracle」驗證

- **隔離 Python 的讀寫位置**：Python 版固定從「腳本所在資料夾」讀舊檔、也寫回那裡，並鏡像到 Lively 夾（B-FLOW-3、
  12、15）。oracle 不改 `update_tw_events.py`：每次執行都把腳本複製到新的暫存資料夾（情境的 `old.json` 放進去當作
  `tw_events.json`），從那份複本 import，把 `lively_wallpaper_dirs` 換成回傳空清單、`sys.argv` 只留腳本本身，執行完
  再從暫存資料夾讀輸出。repo 與使用者的 Lively 資料都不會被寫到。
- **凍結時間**：把複本模組裡的 `datetime` 換成子類別，`now(tz)` 帶 tz 時回傳情境的台北時間，不帶 tz 時回傳情境指定
  的本機時間，讓 B-DATE-2 與 B-DATE-3 兩種用途都可重現；`wait_for_network` 換成立即成功。
- `tests/fetch_oracle/record.py`：以上述包裝跑一輪真實抓取，同時包住 `http_bytes`，把每個請求與回應（含 HTTP 錯誤）
  寫成樣本目錄（對照表＋回應檔），並記下錄製時間。只在錄製時發網路請求。
- `tests/fetch_oracle/expect.py`：讀樣本目錄，以同一份對照表回放，輸出 `expected.json`。
- 樣本目錄放 `host/tests/fixtures/fetch/<情境>/`：`recorded-<日期>`（真實錄製）加上手工構造的邊界情境（對應
  behavior-inventory 的 K 區），從錄製樣本複製後修改，`expect.py` 產出期望輸出，一併提交。
- Rust 測試對每個情境以 `FixtureFetch` 跑一輪，把輸出與期望檔都解析成 `serde_json::Value` 比對相等。浮點數兩邊都是
  「最短往返」寫法，解析後相等。
- **錯誤訊息正規化**：`{e}` 是例外的說明文字，Python 與 Rust 不可能相同。比對前以一份「已知前綴清單」（B-FLOW-9
  帶 `{e}` 的各訊息，例如「處置股來源失敗（上櫃）：」）把命中的訊息截成前綴；清單外的訊息（如「啟動時網路等待逾時，
  改用既有資料兜底」「法說會：無市值前百大名單可篩，本輪改用備援來源」）整句比對。筆數與順序必須相同。
- 刻意改良（D8）的情境沒有 Python 期望檔可產，期望檔手寫，並在情境的說明檔註明。
- `tests/test_wallpaper_data.py` 的 54 個案例對應盤中走勢、日 K、券資比、休市日，Rust 端逐一移植成單元測試。

### D5. 排程：台北時間固定時點＋補抓＋間隔下限

一條背景任務每 60 秒醒來一次，以**系統時鐘**（不是單調時鐘）判斷是否已到下一個時點、或是否錯過了時點。上一輪的開始、
完成時間與「是否有任何來源成功」存在 `%LOCALAPPDATA%\tw.fintools.fc-host\fetch-state.json`（D10），宿主重啟後也知道。睡眠恢復因此不需要另外接系統通知：恢復後下一次
醒來（至多 60 秒）就會發現錯過時點，再加 30–120 秒隨機延遲補抓。`desktop.rs` 目前只處理 `PBT_POWERSETTINGCHANGE`、
沒有恢復通知，本設計刻意不新增。規則見 spec「定時抓取」「啟動與恢復時補抓」「抓取間隔下限」。

- 時點用台北時間而非本機時間：15:00 對應收盤後盤中走勢（dynamic-wallpaper 2.2），人在國外時也應依台股作息。
  原排程是本機時間，台灣使用者沒有差別。
- 60 分鐘下限滿足 ForexFactory 社群說法「每 IP 5 分鐘 2 次」並留餘裕；隨機延遲分散多台機器。
- 排程判斷寫成純函式（輸入：現在時間、狀態、資料檔是否存在、亂數；輸出：下一輪何時開始），以注入的時鐘測試。定義：
  - 「到點」：上一次醒來到這一次醒來之間跨過一個時點，且兩次醒來相隔不超過 3 分鐘 → 排在時點後 0–5 分鐘的隨機時刻。
  - 「錯過」：上一輪開始之後已經過了某個時點，但不是「到點」（宿主剛啟動、或兩次醒來相隔超過 3 分鐘＝睡眠或卡住）
    → 30–120 秒後補抓。
  - 失敗重試：上一輪沒有任何來源成功 → 10–15 分鐘後重試，連續失敗加倍、上限 60 分鐘，不受 60 分鐘下限。
  - 系統時鐘倒退（上一輪開始時間晚於現在）：把上一輪開始時間視為現在，不立即抓。
  - 狀態檔讀不到或壞掉：視為沒有紀錄（等同「錯過」，30–120 秒後抓；資料檔不存在時走 5–15 秒的首抓）。
  - 首抓的熱迴圈防護：同一個資料目錄剛**完成**一輪、檔案仍不存在（例如目錄寫不進去）→ 改走失敗重試的退避；換資料目錄、
    或抓取由停用轉為啟用時解除。被停止而中止的一輪不算「完成」。
  - 排程紀錄寫不進磁碟時，迴圈以記憶體中的紀錄為準，60 分鐘下限與重試退避照常有效。
- 排程任務提供停止介面（關閉時等進行中的一輪在下一個來源邊界停下，有時限），供系統匣「結束」與 installer-auto-update
  的「因更新結束」使用。被停止而中止的一輪不寫檔、不計入 60 分鐘下限（下次啟動視同錯過時點補抓）。

### D6. 流程與模組切分

`host/src/fetch/`：`mod.rs`（一輪的組裝：讀舊檔 → 依 B-FLOW-5 順序呼叫各來源 → 合併、窗口、去重、排序、counts →
`fetched`／`errors` → 原子寫檔）、`http.rs`（D2／D3）、`sched.rs`（D5）、`output.rs`（輸出結構與寫檔）、`dates.rs`
（民國年、台北時間、`round2`、`%g`）、各來源一檔（`macro_ff.rs`、`dividend.rs`、`meeting.rs`、`top100.rs`、
`conference.rs`、`punish.rs`、`quotes.rs`、`sofr.rs`、`holidays.rs`、`twii.rs`、`margin.rs`）。來源仍依序執行（B-FLOW-5），
對同一主機的請求至少間隔 2 秒（研究：社群流傳 TWSE「5 秒 3 次」上限，未見官方數字）；測試用的 `FixtureFetch` 不等待。

### D7. 錯誤隔離

每個來源的錯誤轉成 B-FLOW-9 的中文訊息字串（逐字沿用，`{e}` 部分以 Rust 錯誤的 Display 代入）。**每個來源**的呼叫
（含解析）各自以 `catch_unwind` 包住：panic 視同該來源失敗，走該來源的沿用規則並記錯（動態桌布三鍵用 B-GRD-1 的
「{label}來源發生非預期錯誤：…」），其餘來源照常，該輪照常寫檔。這對應 Python 的 `except Exception` 與
`guarded_wallpaper_fetch`，也是 `tests/test_wallpaper_data.py` 的「非預期例外不使整輪停」。整輪外層再包一層作為最後
防線：外層攔到時不寫檔、記錄錯誤，該輪記為失敗、依 D5 的失敗重試（10–15 分鐘、加倍）再抓。寫檔失敗同樣記為失敗。

### D8. 刻意改良（與 Python 不同，需在測試中標明）

1. 不做憑證不驗證退回（D2）。
2. 總經單列欄位型別異常只略過該列（K-14）；任何來源的非預期錯誤只影響該來源（D7）。差異只出現在 Python 沒有接住
   的少數路徑。
3. 上一份輸出中型別不對的鍵視為不存在；物件陣列鍵（`macro`、`events`、`punish`、`quotes`）逐筆檢查，形狀不符的元素丟棄、其餘保留（Python 只對動態桌布三鍵做型別檢查，其餘鍵會崩潰或原樣輸出壞元素）。
4. 不寫 `tw_events.js`、不鏡像 Lively（B-FLOW-15、16）。
5. 等網路以 HTTPS 主機的 TCP 連線探測（同 B-FLOW-2），但只阻塞抓取任務，不阻塞宿主。
6. 數值欄位的極端型別：布林（Python `float(True)` 為 1.0）、NaN／inf、JSON 的 `NaN`／`Infinity` 字面值（Python `json.loads`
   接受、`serde_json` 不接受），一律視為該來源（或該列）失敗，不產生數值。真實來源不會送這些值，對齊 Python 只會把
   壞資料寫進輸出（task 3.1 審查 I2 的裁定）。
7. 總經 `date` 沒有時區偏移的列略過（B-FF-4 要求 Rust 版自行決定；Python 會把 naive 時間當成本機時間換算，結果依執行
   機器而異）。FF 實際回應一律帶偏移。
8. 行情的 `t`（`regularMarketTime`）只接受整數，其他型別輸出 `null`（Python 原樣輸出字串或小數）。前端不讀這個欄位
   （U-5），Yahoo 一律送整數（task 3.4 裁定）。

其餘一律照 Python，包括已知的怪行為（如備援成功時 `earnings` 事件整批消失，B-CONF-10；`STOCK_DAY_ALL` 一輪下載兩次，
K-17）。要改這些行為另開 change，先讓等價測試鎖住現況。

### D9. 抓取規則不做遠端下發

研究提過「把抓取規則與 exe 分離、獨立更新」。不採用：規則表達力不足以涵蓋 regex、昨收序列、去重等邏輯，等於要在
程式內再寫一個直譯器。自動更新（installer-auto-update）每 6 小時檢查一次，修正可在一天內送達，已滿足「快速更新」。

### D10. 抓取開關、隔離環境偵測、排程紀錄與 `--fetch-once`

- `data_fetch`：`"auto"`（預設）／`"on"`／`"off"`，以 serde 預設值補上，舊設定檔不需遷移、`SETTINGS_VERSION` 不變。
  不認得的值視為 `"auto"` 並記錄。
- `"auto"` 的隔離偵測：以 `SHGetKnownFolderPath(FOLDERID_LocalAppData)` 取系統登記的本機應用程式資料夾，與環境變數
  `LOCALAPPDATA` 比較（兩邊都以 `GetFullPathNameW`／`GetLongPathNameW` 展開、正規化大小寫與結尾斜線）；不同就視為隔離
  環境、不抓取。Win32 呼叫依 AGENTS.md 放在 `desktop.rs`。
  - 只比 `LOCALAPPDATA`、不比 `APPDATA`：資料目錄與排程紀錄都在 `LOCALAPPDATA`；`APPDATA`（Roaming）在企業環境可被
    資料夾重新導向，比它會增加誤判。OneDrive「已知資料夾移動」只搬桌面、文件、圖片，不搬 AppData；本機 AppData 也
    不支援重新導向，環境變數由登入時依登記值設定，正常環境兩者一致。
  - Restart Manager 重新啟動（`RmRestart`）的行程不繼承呼叫端的環境變數（memory
    `restart-manager-rmrestart-ignores-caller-env`），所以它會拿到真實的 `LOCALAPPDATA`、偵測為「非隔離」而開始抓取，
    寫進的是使用者真正的資料目錄——這與它本來就落在真實路徑的行為一致，不是新的污染來源；會用到 RmRestart 的驗收
    腳本（`rm-restart-test.ps1`）若不希望抓取，要在它放入的設定檔寫 `"off"`。
- 為什麼這樣做：驗收腳本約 37 支以環境變數隔離宿主，其中有的放固定樣本、有的刻意從空資料目錄驗「沒有資料」的行為
  （`verify-5.8`、`verify-datadir-empty`、`verify-5.2` 等），多支不寫設定檔。預設開啟的抓取會覆寫或填滿它們的資料目錄，
  改變驗收結果；逐支加設定要動收尾線的大量腳本。隔離偵測讓它們全部不必修改。前一版設計的「資料目錄狀態檔」擋不住
  「空資料目錄」這一類，且會被 `--fetch-once` 寫入的檔案永久卡住，已放棄。
- 代價：環境變數與系統登記不一致的少數真實環境（資料夾重新導向設定錯誤等）會不抓取；記錄檔寫明原因，使用者可設
  `"on"`。收尾線的長跑若在隔離環境下需要真實資料，要在設定檔寫 `"on"`（已通知收尾線）。
- 排程紀錄 `fetch-state.json` 放在 `%LOCALAPPDATA%\tw.fintools.fc-host\`（宿主自己的資料夾，不在資料目錄）：
  `{"last_start": RFC3339, "last_finish": RFC3339 或 null, "last_any_success": bool, "consecutive_failures": int}`。
  切換資料目錄不會重設 60 分鐘下限。
- `--fetch-once <目錄>` 在 `main()` 最前面、單一執行個體與 Tauri 建立之前處理（同 `--restore-wallpaper` 的位置），
  自建 tokio runtime 跑一輪後結束；不看 `data_fetch`、不讀寫排程紀錄。暫存檔名含行程 ID，與常駐宿主同時寫同一目錄時
  不互相破壞。用途：實網對照（task 6.1）、除錯、宿主排程出問題時的手動補救。

### D11. 時間處理

`time` crate：台北為固定 +08:00（B-DATE-1）；本機時間以 `OffsetDateTime::now_local()`（Windows 上可用），失敗時退回
UTC 並記錄。`round(x, 2)` 的實作必須與 Python 一致（K-4），以 Python 實際輸出的邊界值（如 0.125、2.675、1.005）
寫成單元測試。`%g` 自寫（K-6）。

浮點數解析：`serde_json` 開 `float_roundtrip`（整個 crate 生效）。預設的快速解析會讓少數小數（例如
`0.0025900000000000922`）差 1 ulp，與 Python 的 `float()` 不同、oracle 比對失敗；開啟後解析結果與 Python 一致。宿主其他
JSON（設定檔、桌布設定）只會更精確，解析速度的差異可忽略。

### D12. 資料來源標示

政府資料開放授權條款要求標示來源（研究 data-layer-tech.md §4.3）。設定視窗底部加一段固定文字與條款連結（spec
「標示資料來源與抓取狀態」）；連結以系統預設瀏覽器開啟，沿用設定視窗既有的外部連結作法（若沒有，就只顯示網址文字）。
同一區塊顯示抓取狀態（上一輪完成時間、是否有來源失敗、或「未抓取」與原因），資料取自排程紀錄與目前的開關判定，
經設定視窗既有的查詢指令取得（收尾線審查建議：真實使用者被隔離偵測誤判時不會無聲停擺）。

## Risks / Trade-offs

- [非官方來源的條款與限流：Yahoo 條款禁止自動化存取、MOPS 的 robots 全禁、ForexFactory 限流] → 每台機器自行抓、
  每日 5 輪加隨機延遲、60 分鐘下限、同主機間隔 2 秒，量級與個人瀏覽相近；來源被封鎖時只有該來源沿用舊資料。改用授權
  較清楚的端點（例如除權息改以 openapi `TWT48U_ALL` 為主）列入 backlog，因為會改變輸出，不在本 change 做。
- [安裝量增加後來源開始擋] → 錯誤訊息會在 `errors` 中累積；靠自動更新換來源。
- [Python 等價測試只涵蓋樣本見過的形狀] → 邊界情境依 K 區逐條構造；實網對照（`--fetch-once` 與 Python 同時跑、
  比對計數與鍵）在合併前跑一次。
- [錄製樣本含真實市場資料、會過期] → 樣本只用在凍結時間的測試；不在測試中呼叫網路。
- [宿主沒在執行就不更新] → 開機自啟由安裝檔登錄；資料過期時小工具已有「已 N 天未更新」提示。
- [`now_local()` 在少數環境取不到時區] → 退回 UTC 並記錄；影響只有 `updated`／`fetched` 的顯示。
- [Restart Manager 或 panic 結束宿主時，寫檔中斷] → 暫存檔加改名，讀取端不會讀到半份；殘留的暫存檔在下一輪覆寫。
- [隔離偵測誤判（真實環境的環境變數與系統登記不一致）] → 記錄檔寫明；使用者可設 `data_fetch: "on"`。

## Migration Plan

1. 本 change 合併後，新宿主自己抓資料；`setup.bat`、排程器、`update_tw_events.py` 只服務凍結的 Lively 版。
2. 開發機的宿主若把 `data_dir` 指向 Python 產檔的資料夾（例如 `D:\finance-calendar`），要把 `data_fetch` 設為 `"off"`，
   否則兩邊會輪流寫同一個檔。
3. 退回：`data_fetch` 設為 `"off"`，再以 Python 產檔到 `data_dir`。
4. dynamic-wallpaper 的「每日 15:00 更新」以 MODIFIED delta 改由本 change 的 15:00 時點提供；該 change 的 tasks.md 2.5
   註明被取代。歸檔順序：desktop-widget-host → dynamic-wallpaper → 本 change。
