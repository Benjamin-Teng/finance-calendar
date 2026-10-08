## Context

動機見 proposal.md。現況與限制：

- 宿主 `host/`（Rust／Tauri 2，desktop-widget-host change）已有：Win32 集中於 `desktop.rs`；資料以 `JsonFileSource` 輪詢
  `tw_events.json`、經 `tauri::ipc::Channel` 推給依 webview label 決定的訂閱者（該 change 的 design.md D4）；
  `desktop.rs` 的 gatekeeper 偵測鎖定、顯示器關閉、全螢幕／簡報（`SHQueryUserNotificationState`）、電池與省電，
  `widgets.rs` 彙整成暫停原因並套用 `PauseRules`；系統匣與設定視窗在 `tray.rs`。
- 離開 Lively 的失效鏈（專案 memory `lively-explorer-crash-chain.md`）：問題在「explorer 對桌布行程做跨行程同步呼叫」。
  本 change 反過來是「宿主呼叫 explorer」，風險面變成宿主被 explorer 拖住，以及高頻設桌布對 explorer 的長期負擔。
- 桌布系統 API 已查證（Microsoft Learn `IDesktopWallpaper`）：可逐螢幕 `SetWallpaper`；`SetPosition` 為全域；已拔除的
  螢幕仍會被列舉，以 `GetMonitorRECT` 回 `S_FALSE` 辨識（實機回的是 `E_FAIL`，見 D4）。未載明而需探針實測者見 Open Questions。
- 探索期的樣稿與完整決策紀錄在 `docs/SPEC-dynamic-wallpaper.md`（git 不追蹤）與 `assets/design-explore/`（git 忽略）；
  實作時需要的內容（星盤樣稿、時段表初值、圖示 SVG）要搬進 `host/` 正式追蹤。

## Goals / Non-Goals

**Goals:**

- 桌布更新對 explorer 的負擔可量測且有上限：最頻繁的主題（星盤）每 15 分鐘每螢幕一次。
- 宿主任何執行緒都不會因 explorer 無回應而卡住超過逾時。
- 繪圖頁是純函式：同樣的尺寸、時間、資料、設定 → 同一張圖，可在宿主外以瀏覽器截圖測試。
- 排程與狀態判定（何時重畫、是否讓位、還原什麼）是不碰 Win32 的純邏輯，可單元測試。

**Non-Goals:**

- 連續動畫或滑鼠互動的桌布；把任何視窗嵌入或置於桌面圖示層。
- 為每台螢幕選不同主題；橫跨多螢幕的單張構圖。
- 安裝檔與解除安裝流程（子專案 3，只提供 `--restore-wallpaper` 給它呼叫）。
- 爬取各交易所休市日（改由可編輯年度表）。

## Decisions

### D1 定時出圖＋系統桌布 API，不開置底全螢幕視窗

以系統 API 設定圖片桌布，桌面圖示、explorer 行為完全不變。替代方案：全螢幕置底 WebView 視窗（可連續動畫，但會蓋住
桌面圖示或必須進 WorkerW，正是要離開的模式）、`SystemParametersInfo(SPI_SETDESKWALLPAPER)`（無逐螢幕參數）。
代價：更新頻率受限，星盤因此改為 15 分鐘一格（呼應 proposal 的初衷）。

### D2 隱藏 WebView2 渲染，繪圖頁回傳 PNG 位元組

`wallpaper.rs` 建立一個隱藏、不出現在工作列的渲染視窗（label 前綴 `wallpaper-renderer-`，每次渲染一個
`wallpaper-renderer-<render id>`；判斷是否為渲染視窗一律比對前綴，不比對固定字串），依序為每台螢幕載入主題頁、帶入
`w`／`h`／`t`（絕對 ISO 時刻，含 `Z`；不是 `now`，頁面對未知參數會在狀態裡記警告）／`tz`。頁面透過通道拿到資料與主題設定、
畫完 canvas 後**由頁面自己**以 `invoke` 回報（Tauri `eval` 沒有回傳值，宿主讀不到頁面狀態）：成功呼叫
`wallpaper_render_done`，以 PNG 原始位元組為請求本體（Tauri 2 支援二進位請求本體，避免 base64 膨脹）、meta 放 header；
失敗呼叫 `wallpaper_render_failed`（錯誤原因＋meta）。畫完但帶警告仍算成功，警告放 meta 供記錄。頁面沒有機會回報
（模組載入失敗、卡住）時由宿主的渲染逾時涵蓋。引數形狀以 `host/ui/wallpapers/lib/wallpaper.mjs` 檔頭「宿主接法」為準。頁面不使用計時器與 `requestAnimationFrame` 等待（隱藏頁可能被節流），字型以
`document.fonts.load` 確保載入後才畫。替代方案：Rust 繪圖庫重寫五套（畫風與中文排版難一致）、Python 資料層出圖
（破壞純 stdlib 原則，且資料層之後要進 Rust）。

**渲染視窗每次開、畫完即關，且使用獨立的 WebView2 環境**（探針 1.4，`host/tools/evidence/dw-1.4-*`）。4K 星盤實測
220 次全部成功、同一 `t` 逐位元組相同。各方式的數據：

- 每次開關：中位數 0.51–0.74 s。
- 常駐、每次重新導覽：每次閒置記憶體約 +30 MB、不收斂，淘汰。
- 常駐、同一文件重畫：0.21 s。

在共用 WebView2 環境（小工具常駐）下，「每次開關」與「同文件重畫」第 40 次的閒置記憶體都約 590 MB，每次約 +3 MB，
關閉後共用行程仍比基準多約 200 MB，所以共用環境下哪一種都不會回到基線。選「每次開關」的理由是：故障隔離——逾時或卡住
直接銷毀視窗、保留舊圖；頁面不必另外提供重畫入口。記憶體歸還則靠獨立環境：渲染視窗用自己的 user data folder，於是有
自己的 browser 行程，畫完關閉、整個 browser 行程結束。這也是 desktop-widget-host run3 判準（宿主＋全部 WebView2 私有記憶體
增幅 < 20%）能成立的前提。

獨立環境的可行性與冷啟動成本、殘留是否每次累加、15 分鐘頻率下 2 小時的記憶體曲線，由 4.5 在真實宿主實測並回報收尾線；
若殘留無法避免，run3 判準由使用者決定。

其他 1.4 結論：

- 頁面回傳一律是原始位元組。JSON 陣列路徑解碼結果相同，但慢約 12 倍，只當相容退路。
- meta header 實測最大 191 B。
- 回應必須帶 render id 比對，防止逾時後晚到的 PNG 被算給下一次。
- `toBlob` 在 Tauri `visible(false)` 下不延後。
- 鎖定、顯示器關閉、待機狀態下的渲染行為未測，留給 4.5／6.1。

### D3 `IDesktopWallpaper` 放在專屬 COM 執行緒，呼叫端有逾時

`desktop/wallpaper.rs` 新增一個專屬執行緒（`CoInitializeEx` 一次、持有 `IDesktopWallpaper`），以佇列接收「列舉螢幕／讀取／設定／
還原」請求，結果經 channel 回傳；呼叫端以逾時等待（預設 10 秒）。逾時時該次視為失敗，並把執行緒標為「忙碌」——
在前一個請求返回前不再送新請求，排程照常推進到下一個時點（例外：COM 忙碌解除或 explorer 重啟處理完畢時，上次為套用
失敗的螢幕可提早重試一次，每台每個時點至多一次；渲染失敗另走退避重試，見 `host/src/wallpaper.rs` 的呼叫端契約）。
explorer 有回應但回報失敗（設定或設定前讀回回錯）不是「卡住」，不會有忙碌解除事件，改由排程器依渲染失敗的同一條退避
曲線重試（2026-10-06 在場驗收修正）；等待期間的決策記為「等待重試」，不記成已是最新。已知落差：設定**逾時**的
螢幕沒有重試時刻（等忙碌解除或下一個時點），排程器列不出它，等待期間的決策仍記為「已是最新」；此時實情由設定視窗
狀態的 COM 忙碌旗標與記錄檔的「設定桌布失敗……記錄 Apply」那一行呈現，忙碌解除事件會提早重試。
主執行緒與 Tauri 指令處理緒都不直接等待它。
替代方案：每次呼叫臨時開執行緒（卡住時會無限累積執行緒）、在主執行緒呼叫（違反初衷）。

探針 1.1 實測 `SetWallpaper` 1,440 次全部 `S_OK`，耗時中位數 1.5 ms、p99 2.6 ms、最大 10.3 ms
（`host/tools/evidence/dw-1.1-full.log`）。呼叫在 explorer 實際處理（每次約 160 ms CPU）之前就返回，所以 `S_OK` 只代表
請求被接受，是否生效以 D5 的讀回為準。10 秒逾時只為 explorer 卡住而設，數值不變；耗時超過 1 秒時記警告，作為
explorer 異常的早期訊號。

### D4 輸出檔、填滿方式與螢幕識別

每台螢幕在 `%LOCALAPPDATA%\tw.fintools.fc-host\wallpaper\` 交替寫 `<螢幕鍵>-a.png`／`-b.png`（兩檔交替即可，
不累積）。當初擔心同路徑重設不觸發重新載入；6.1 B10 實機證實同一路徑覆寫內容後再設定，explorer **會**重新
載入，a／b 交替無害、保留。螢幕鍵沿用 desktop-widget-host 已有的穩定顯示器識別（task 2.5，
`DisplayConfigGetDeviceInfo`），並以 `GetMonitorRECT` 的座標與 `GetMonitorDevicePathAt` 的裝置路徑對應；對應不到時退回
裝置路徑的雜湊。每次重畫都重新列舉，略過離線者。2026-10-06 在場驗收實機結論：離線螢幕仍出現在
`IDesktopWallpaper` 列舉中（`GetMonitorDevicePathCount` 不減、路徑與索引不變）、`GetMonitorRECT` 回 `E_FAIL` 而不是
文件寫的 `S_FALSE`；離線判定以 RECT 失敗為準，不以列舉數量。列舉逐台處理：有不在線的項目（RECT 回 `S_FALSE` 或失敗）、
而在線台數已達系統作用中的顯示器數（`GetSystemMetrics(SM_CMONITORS)`，在逐台查詢之後讀）時，只讓那幾台算離線，其餘
照常；在線台數不足＝不在線者至少一台是作用中的（`RPC_E_CALL_REJECTED` 之類的暫時錯誤、拓樸切換途中、插回後 explorer
落後於顯示變更事件——落後時回 `E_FAIL` 或文件記載的 `S_FALSE` 都可能），整批回錯、退避重試，不當成離線（否則還原會把
它標成待還原、命令列回「仍有離線」，解除安裝後那台永久停在宿主的圖；插回時會被當成已列舉完畢而不再補畫）。只比台數、
不對應「哪一台」：系統的顯示器與 `IDesktopWallpaper` 的裝置路徑沒有可靠的一對一鍵，台數足以證明不在線者都不是作用中的。
沒有任何不在線的項目時不看系統台數（`IDesktopWallpaper` 若漏列某台作用中的顯示器，列舉仍照常回傳、不會永久失敗）；
讀不到系統台數時，只有失敗 HRESULT 整批回錯，`S_FALSE` 照舊算離線。介面斷線類 HRESULT、或沒有任何在線螢幕而有失敗
HRESULT 時同樣整批回錯。列舉失敗而沿用上次清單時，不以舊座標出圖與設定，並依渲染
失敗的同一條退避曲線再列舉（沒有要重畫的螢幕時也一樣，決策記為「等待重試」）；待還原螢幕的處理失敗同樣退避重試
（插回後的那次顯示變更事件之後不一定還有事件）。首次接管時把全域填滿方式設為 `DWPOS_FILL`（圖與螢幕同尺寸時為 1:1），原值寫入
狀態檔供還原。尺寸取 `GetMonitorRECT`，它是實體像素、不必換算。

task 1.5 實機結論（2026-10-06 使用者在場）：

- **識別**：`GetMonitorDevicePathAt` 的字串與 `QueryDisplayConfig` 的 `monitorDevicePath`（task 2.5 穩定顯示器識別的來源）
  逐字相同，依路徑與依 RECT 對應的結果一致。切換縮放（150%→175%→150%）、變更解析度、兩次拔插外接螢幕前後，每台的
  路徑與索引都不變。
- **尺寸**：`GetMonitorRECT` 不受行程 DPI 感知影響——執行緒預設 DPI unaware 與 Per-Monitor-V2 讀到的 RECT 相同；
  改縮放時 RECT 不變，只有改解析度（3840×2160→2560×1600）才變。
- **拔除**：該台仍在列舉中、`GetMonitorRECT` 回 `0x80004005`（`E_FAIL`），插回後恢復原 RECT 與 `S_OK`；離線判定照上文
  的台數佐證。
- **證據**：`host/tools/evidence/dw-1.5-segment1-summary.txt`／`dw-1.5-segment1.jsonl`（縮放、解析度、一次拔插）、
  `dw-offline-inperson-dw15.log`（另一次拔插）、`dw-1.5-scale-monitor-ids.log`（縮放前後的路徑與 RECT）。

### D5 狀態檔先寫後設、原圖備份、讓位判定

`wallpaper-state.json`（宿主設定資料夾）記錄：是否接管、各螢幕原桌布（圖片路徑＋備份檔名／純色色碼／投影片來源與
間隔）、原填滿方式、各螢幕上次設定的路徑。任何設定動作前先原子寫入狀態檔。以下規則見 task 6.4：

- **原圖備份**：首次接管時把原圖複製到 `wallpaper\original\`；來源沒有圖片副檔名時，備份的副檔名依內容判斷
  （認 PNG／JPEG／BMP／GIF／WebP 簽章，判斷不出來用 `.jpg`）。原圖複製不了（接管前已被刪除——explorer 仍以快取顯示它——
  或權限、磁碟問題）時改備份 explorer 的轉存檔，依**設定方式**選來源（6.1 實機與重跑：逐螢幕設定時每台有自己的
  `%APPDATA%\Microsoft\Windows\Themes\Transcoded_<索引三位數>`，編號＝第一次設定前列舉時的 `GetMonitorDevicePathAt`
  索引，**格式與尺寸沿用原圖**；「全部螢幕」設定 `SetWallpaper(NULL, 圖)` 不產生逐螢幕轉存檔，只更新全域
  `TranscodedWallpaper`；逐螢幕設定過之後全域檔不代表任何一台，B3 拿它當備份還原成錯的圖；全域純色會刪掉逐螢幕檔）。
  設定方式以第一次 `SetWallpaper` **之前**的登錄 `Wallpaper` 快照判定（修正輪 4：「逐螢幕檔存不存在」不可靠——
  逐螢幕設定過再改「全部螢幕」設定時，舊的逐螢幕檔可能還在；實機逐螢幕設定後登錄是 `...\Themes\TranscodedWallpaper`，
  「全部螢幕」設定後是原圖路徑）：
  - 登錄值（`REG_EXPAND_SZ` 先展開）＝各在線螢幕讀回的共同路徑（不分大小寫、`pathcmp` 正規化）→ 全部螢幕設定 →
    用全域 `TranscodedWallpaper`，多螢幕也可以，即使殘留逐螢幕檔也不用；
  - 登錄值是這個 Themes 資料夾的 `TranscodedWallpaper`（路徑相同或檔案識別相同）→ 逐螢幕設定 → 用該螢幕的
    `Transcoded_<索引>`，不存在就不能備份（不退回全域檔）；
  - 其他（登錄讀不到、空字串、與讀回不符又不是轉存檔路徑）→ 不能備份。

  選中的檔必須存在、可讀、大小 > 0，且依檔頭判斷得出是圖片，副檔名照檔頭；**不**核對格式或尺寸（修正輪 3：先前
  「PNG＋螢幕解析度」的推論被實機推翻，照片與非螢幕尺寸的原圖會一律備份失敗）。仍備份不了就不接管（主題改
  「不接管」並通知）。接管中新接上的螢幕原圖備份不了（轉存檔此時已是宿主的圖，不能代替）就停止整個接管、
  還原其他螢幕；它的讀回本身就是轉存檔時，記為原桌布未知、不備份，還原時改用其他螢幕的原桌布或登錄記錄的原值。
- **讓位判定**：設定前以 `GetWallpaper` 讀回，若既非上次所設路徑、也非宿主輸出資料夾內的檔案，即判定使用者自換。
  例外是讀回為 explorer 的桌布快取工作檔：`%APPDATA%\Microsoft\Windows\Themes\` 下的 `TranscodedWallpaper`、
  同一層 `Transcoded_` 開頭的逐螢幕轉存檔、`CachedFiles\` 與 `TranscodedWallpaperCache\` 子資料夾內的檔
  （explorer 會改寫，內容可能已是宿主的圖；6.1 B6 實機用的是 `TranscodedWallpaperCache\`，`CachedFiles\`
  沒出現過）。這時無法判定：不讓位、不改原桌布紀錄，下次再判；還原時也不算使用者自選。使用者套用的佈景主題
  （`%LOCALAPPDATA%\Microsoft\Windows\Themes\<名稱>\DesktopBackground\`）不是快取，照常讓位。6.1 B2／B6 實機：
  `GetWallpaper` 讀回從未落在任何快取檔、以 `\\?\` 設定的讀回會去掉前綴、8.3 短檔名原樣回傳——短檔名只在別的
  程式以短檔名設定時出現（那本來就是使用者自換），路徑比對不另外展開短檔名。explorer 重新啟動、登出再登入後
  的讀回仍待使用者在場驗（6.1 B1）。
- **純色原桌布**（6.1 B9 實機）：純色是**全域**狀態——`SetWallpaper(<螢幕>, "")` 不會變純色，而是換回 explorer
  記憶中的逐螢幕圖片（仍回 `S_OK`）；只有 `SetWallpaper(NULL, "")` 才是純色（此時 `GetStatus` 回 0）；純色狀態下
  只設一台圖片，其餘螢幕的讀回立刻變成記憶中的圖片。因此：接管前所有在線螢幕都讀回空字串＝原桌布記為全域純色
  （含背景色），但 Windows 焦點啟用中（`EnabledState`＝1）時不算純色、照焦點流程；接管途中，**還沒設定過**的
  螢幕讀回變成某張圖片不算使用者自換（設定過但沒生效、讀回仍是設定前記下的同一張記憶圖片也算；設定確定失敗——
  HRESULT 錯誤或請求沒送出——時 `last_set` 退回設定前的值，視為沒設定過，逾時則不退；已設定的螢幕讀回換成別的圖
  照常讓位，比「接管完成前一律不讓位」更窄）；還原時沒有使用者自選的螢幕就以一次 `SetWallpaper(NULL, "")` 還原，再寫回
  背景色、填滿方式與登錄，不用逐螢幕空字串。有使用者自選的螢幕時無法只讓其餘螢幕回到純色，逐螢幕設空字串、
  讀回已不是宿主的圖就算還原。還原當下離線、標為待還原的純色螢幕重新接上時不單獨設空字串：其餘在線螢幕仍是
  純色才再呼叫一次 `SetWallpaper(NULL, "")`，否則（使用者已改選圖片）不動它、清掉待還原，記錄寫明它仍顯示宿主的
  圖、之後不會再自動還原、需使用者自行更換（結果另列，不算「已不是宿主的圖」）。
- **被取代的備份**：讓位或還原時發現使用者自換、紀錄改採新桌布，舊備份保留到下一次接管開始才清（判定若是誤判，
  原圖仍在）。
- **還原來源**：原路徑存在就用原路徑、不存在就用備份；原路徑是上述快取工作檔、或檔案識別與記錄時不同（檔案被
  換掉）而有備份時，改用備份。
- **系統匣通知**：Windows 通知橫幅只顯示標題下約 4 行內文（6.1 A9 實機），結果（已停止接管＋是否已還原、為何
  沒接管）放第一句、整則控制在 4 行內；讀值、門檻、路徑等細節只寫記錄檔。

Windows 焦點的偵測為盡力而為：偵測得到才提醒，偵測不到時視同一般圖片處理。官方沒有可查詢焦點的 API
（見 Open Questions「已解答」），故讀 `HKCU\Software\Microsoft\Windows\CurrentVersion\DesktopSpotlight\Settings` 的 `EnabledState`＝1 判定；機碼與
`Creatives` 在停用後仍殘留，不可用「機碼存在」判定。接管前另把 `EnabledState` 與 `Explorer\Wallpapers` 的
`BackgroundType` 原值記入狀態檔供診斷。

### D6 排程器為純函式，事件只負責喚醒

`wallpaper.rs` 的核心是 `next_action(now, theme, state, pause, data_dates) -> Action`（何時重畫、重畫哪些螢幕、或讓位、
或等待資料），不碰 Win32 與檔案。喚醒來源：單一計時器（設在下一個時點）、資料通道變更、螢幕變更與 DPI 變更（沿用
`desktop.rs` 既有的顯示變更處理）、暫停原因變化（沿用 `widgets.rs` 彙整的暫停原因與 `PauseRules`，不另寫偵測）、
設定變更。暫停期間只記錄「錯過的時點」，恢復時補畫一次。

### D7 資料與設定的流向

新增 `wallpaper` 通道：由 `tw-events` 快照擷取桌布所需的鍵（盤中走勢、日 K、融資、休市日、法說會），加上主題設定檔
內容，只推給 label 前綴為 `wallpaper-renderer-` 的渲染視窗（其他 webview 查詢、訂閱一律拒絕），延續 D4（desktop-widget-host）
「資料本體只走通道、不用事件廣播」。`tw-events` 有新快照或設定檔內容改變（與資料同一個 30 秒輪詢）時重算並推給存活的渲染視窗。主題
設定檔 `wallpaper-config.json` 放宿主設定資料夾；內建預設值編進執行檔，讀檔後逐鍵合併，缺鍵或型別錯以預設補齊並
記錄警告。通道 payload 形狀為 `{ data, config }`（`data`＝擷取的鍵，`config`＝合併後的主題設定；頁面也接受裸
tw_events JSON，視為 `{ data: 它, config: {} }`），頁面對 `config` 缺鍵仍以自己的內建預設補齊。時段表與休市表的時區換算、夏令時間一律在頁面以 `Intl` 依 IANA 時區計算，Rust 端不處理時區規則。

### D8 繪圖頁結構

`host/ui/wallpapers/` 下每套一個頁面＋共用模組（尺寸基準 `S = min(W,H)/2160`、決定性亂數、字型載入、與宿主的通道
介面）。頁面也接受 query 參數（`w`、`h`、`t`、`tz`、`fixture`）以便在宿主外用 headless Edge 截圖測試。星盤以樣稿
`bg-sessions.html` 的 full 構圖為基礎；其餘四套由提案 `bg.html` 把亂數種子換成資料。字型（Cinzel 500／600／700、霞鶩文楷 TC 400／700、
Noto Serif TC 900、Noto Sans TC 500／600、IBM Plex Mono 600，皆 OFL；Plex Mono 供等高線樣稿使用，Noto Sans TC 是星盤的中文字，
取代樣稿字型堆疊中的系統字型微軟正黑體）放 `host/ui/wallpapers/fonts/` 並附授權檔，
以 `@font-face` 相對路徑載入；共用模組位於 `host/ui/wallpapers/lib/`（`core.mjs` 純邏輯可在 Node 測試、
`wallpaper.mjs` 負責字型載入、資料與「畫完」訊號 `window.__wallpaper.phase`）。

### D9 圖示產製與切換

統一圖示與五套主題圖示的 `.ico` 在開發時由 SVG 產生（headless Edge 逐尺寸點陣化、以零安裝的 Node 指令稿封裝成
PNG 內嵌式 ICO），產物進 git，建置不依賴產生工具。exe 內嵌統一圖示（沿用 `build.rs` 現有 `window_icon_path`）；
系統匣與設定視窗在主題變更時於主執行緒呼叫 Tauri 的 `set_icon`。小尺寸（≤24px）用專用的簡化 SVG。

### D10 資料層擴充

`update_tw_events.py` 新增：盤中走勢與日 K（Yahoo v8 chart，與行情條同源；`interval`／`range` 精確值實作時查證）、
上市融資融券（TWSE `rwd/zh/marginTrading/MI_MARGN`，`selectType=MS` 取彙總、`ALL` 取個股張數）搭配既有的
`STOCK_DAY_ALL` 收盤價。沿用既有韌性模式：各鍵獨立失敗沿用舊值、錯誤累積。`scripts/setup.ps1` 加 15:00 觸發。

### D11 explorer 資源安全閥

探針 1.1 實測反覆設桌布會使 explorer 的 GDI／USER 物件累積且不釋放（見 Risks 第一條），降低頻率只延後、不保證有界，
因此由宿主監看並在超過門檻時自行退出（使用者 2026-10-02 決定）。`desktop/explorer_gdi.rs` 以殼層視窗（`GetShellWindow`）的擁有者
PID，`OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)`＋`GetGuiResources(GR_GDIOBJECTS)` 讀數；這是核心查詢、不送訊息給
explorer，不會因 explorer 卡住而阻塞；由協調迴圈在評估結果為重畫、確定要渲染時讀一次（不是每台螢幕設定前各讀一次）。基準值與 explorer PID 記在
`wallpaper-state.json`：接管開始、PID 改變、使用者重新開啟接管時重設；宿主重啟但 PID 未變時沿用。是否停止由
`next_action` 依讀值決定（純函式、可單元測試）：增量 > 2,000 或絕對值 > 8,000（單一行程上限 10,000）即走「不接管」的
還原路徑並通知；尚未接管時讀值已超過 8,000，也不開始接管（主題改「不接管」並通知）。讀取失敗只記錄、不停止。門檻為內建常數，1.7 受控重測後再調整。

### D12 勿打擾偵測

spec「暫停更新」的勿打擾不經 `widgets.rs` 的暫停原因（全螢幕偵測 `SystemBusy` 不涵蓋它，也不該讓小工具一起停），
由 `wallpaper_dnd.rs` 另外偵測（使用者 2026-10-03 決定，task 4.7c）。兩個來源，**任一為「是」就暫停**（OR）：

- **官方**：WinRT `Windows.UI.Shell.FocusSessionManager`，`IsSupported` 為真時讀 `GetDefault().IsFocusActive`。
- **非官方**：WNF 狀態 `WNF_SHEL_QUIETHOURS_ACTIVE_PROFILE_CHANGED`（state name `0x0D83063EA3BF1C75`），以
  `ntdll!NtQueryWnfStateData` 讀 4 位元組整數，值 > 0 視為勿打擾。這是未公開 API（Workrave、YASB 等開源專案採用），
  以 `GetProcAddress` 動態取得，不靜態連結。

任一來源讀不到（`IsSupported` 為假、WinRT 失敗、NTSTATUS 非 0、資料不足 4 位元組、函式不存在）就把那一半視為
「未知」，不因此暫停；兩者都讀不到時照常重畫。WNF 的失敗模式：Windows 更新可能改變 state name 的語意、資料格式或
移除這個匯出，結果是退化成只靠官方半邊。最壞情況是勿打擾時仍照常重畫。最後讀值只在 3 個輪詢週期（180 秒）內
有效；過期後第一次判定先要求輪詢執行緒立即重讀、最多等 2 秒。等到就照新值判斷：計時用的單調時鐘（QPC）含睡眠
時間，睡眠超過 3 分鐘喚醒時讀值看起來過期，但輪詢執行緒只是還沒輪到。等不到（讀取卡在沒有回應的跨行程呼叫裡，
或輪詢執行緒消失）才視為未知、不暫停，同一段過期只等這一次。所以偵測故障不會讓桌布永遠不畫；協調迴圈本來每
60 秒就會再評估一次，不另排計時器。

系統呼叫在 `desktop/dnd.rs`。協調迴圈只在評估結果是 `Redraw` 時查詢，所以主題為「不接管」時不建立輪詢執行緒、
也不做任何讀取。第一次查詢時建立 `fc-wallpaper-dnd` 執行緒，在該執行緒上以 MTA 初始化 COM／WinRT，不影響
D3 的 COM 執行緒與主執行緒；查詢端最多等 2 秒拿第一次讀值，避免剛啟動或剛選主題時先畫一張。之後查詢只讀快取；讀值過期那一段的第一次查詢例外，會再等一次重讀（同一上限）。
輪詢執行緒每 60 秒讀一次兩個來源，主題改為「不接管」就停止輪詢。合併結果改變才送 `DoNotDisturbChanged` 喚醒，
解除時由排程器補畫最近錯過的時點一次（同 D6）。官方半邊雖有 `IsFocusActiveChanged` 事件，但 WNF 半邊仍要輪詢，
兩半一起輪詢比較簡單。

## Risks / Trade-offs

- [長期反覆設桌布使 explorer 累積 GDI／USER 物件] → 探針 1.1 實測（兩台螢幕每 10 秒、2 小時 1,440 次，
  `host/tools/evidence/dw-1.1-full.csv`、`dw-1.1-full.log`）：GDI 351→595、USER 361→約 481，冷卻 10 分鐘不回落；
  GDI 增量以「每次設定 1–3 個」為單位，只出現在約 10% 的時間，觸發條件未明；連跑期間使用者全程不在電腦前（公出），
  可排除使用者操作造成的累積。工作集、私有位元組、CPU 無累積
  （每次設定約多耗 explorer 160 ms CPU，事後回到基線）。全部歸因於設桌布並以平均速率外推，實際頻率（每天 192 次）
  約 300 天才到單一行程 GDI 上限 10,000；若每次都以觀察到的最大速率累積，約 17 天。降頻只延後、不保證有界，故對策
  改為：維持 15 分鐘，以 D11 安全閥監看並在超過門檻時停止接管、還原原桌布並提示。
  1.7 受控重測（2026-10-03，同一個 explorer 行程、只設主螢幕每 10 秒、6 小時 2,160 次、每次設定後 5 秒取樣、使用者全程
  不在且無輸入，`dw-1.1-1.7-full.*`、`.superpowers/sdd/tasks/probe-1.7-analysis.md`）：GDI 991→993（只有第一次設定 +2，
  其後 8 次暫時 +4／+6 皆在 10 秒內回落）、USER 701→695，explorer 頂層視窗數全程不變。結論：**會飽和**——在此 explorer
  狀態下以兩張固定圖反覆設定不再累積；**與使用者操作無關**——1.1 與 1.7 都無輸入，一次累積、一次不累積。1.1 的累積與
  1.7 的平坦之間，explorer 從 595 漲到 991（期間有 6.1 實機驗收的多次兩台設定與一般使用），差異可能來自「兩台同時設定」
  或「已到水位」，兩者未能分離。產品實際是每 15 分鐘兩台各一張新內容的圖，以 6.2 的 24 小時曲線為準。**D11 門檻維持不變**
  （增量 >2,000 或絕對值 >8,000）：以目前約 990 的水位，增量門檻約在 3,000 觸發，距單一行程上限 10,000 仍遠。
- [設桌布會廣播設定變更給所有視窗，某些程式處理不當] → 探針 1.2 已排除（`host/tools/evidence/dw-1.2-full.log`）：
  1,440 次 `SetWallpaper` 加上還原，隱藏頂層視窗收到 0 則訊息，同一視窗確實收得到對照用的 `WM_SETTINGCHANGE` 廣播。
  未涵蓋監看登錄或殼層內部通知的程式。約束：宿主不得自行補送 `WM_SETTINGCHANGE`，還原也不得改用帶
  `SPIF_SENDCHANGE` 的 `SystemParametersInfo`。
- [Windows 備份會備份桌布與填滿方式（1.6 已證實），接管期間的輸出圖可能被帶到新裝置] → 官方未載明上傳的是原檔、
  轉存檔還是路徑，輸出路徑無從避開；影響限於「在新裝置設定時還原出一張星盤圖」，該機未裝宿主時不會更新。設定視窗
  在選主題處加一行提示，說明可到「設定 > 帳戶 > Windows 備份 > 記住我的喜好設定」關閉個人化備份。宿主不改此設定。
- [隱藏 WebView 被節流導致渲染逾時] → 頁面不依賴計時器（D2）；30 秒逾時保留舊圖。
- [螢幕識別碼在插拔後改變，導致原桌布記錄對不上] → 1.5 實機：兩次拔插、縮放與解析度變更前後路徑與索引都不變
  （見 D4）；未測的情境（例如換接埠、換顯示卡）仍可能對不上，對不上時把該螢幕視為新螢幕（記錄當下桌布），最壞情況
  是還原成接管期間的某張圖而非最初那張——以原圖備份＋提示降低影響。
- [字型使安裝檔增加約 55 MB（43 MB＋星盤 Noto Sans TC 約 12 MB）] → 使用者已接受；不做子集以免可編輯詞庫缺字。
- [交易所時段每年會變] → 時段表可編輯；CME、LSE 已於 1.6 以官方現行頁重驗（見 Open Questions「已解答」）。
- [券資比、維持率為社群算法，非官方發布] → 規格明定算法與範圍；門檻可調。
- [使用者自換桌布後，宿主要到下一次重畫前讀回才讓位] → Windows 換桌布不廣播（探針 1.2），讓位只在每次設定前判定；
  星盤延遲最多 15 分鐘，每日型主題（脊線、等高線、天際線、撕日曆）可能延到隔天重畫時點，期間使用者的新桌布不會被
  覆蓋，但沒有通知、設定視窗仍顯示原主題（2026-10-04 使用者實測）。使用者決定先維持現狀；後續可改為接管期間每 60 秒
  以 COM 執行緒輕量讀回、偵測到即讓位（backlog）。
- [離線判定只比台數的殘留情境] → D4 的「在線台數 ≥ 系統作用中顯示器數」只在列舉裡有不在線項目時生效。新接上的螢幕
  若 explorer 還沒把它列入 `GetMonitorDevicePathCount`（不是「列著但讀不到矩形」），而且沒有其他不在線的項目，列舉看起來
  完整、照常回傳，那台要等下一個顯示變更、DPI 變更或 explorer 重啟事件才補畫或補還原（重畫時點不會重新列舉）。clone（複製）顯示模式、遠端桌面
  （RDP）工作階段、IddCx 虛擬顯示器下 `IDesktopWallpaper` 列舉幾台與 `SM_CMONITORS` 的對應都未實測；推論上複製模式的
  在線台數只會大於等於 `SM_CMONITORS`，不受影響，其餘待實機確認。

## Migration Plan

1. 本 change 在 desktop-widget-host 合併進 main 後，從 main 開 `feat/dynamic-wallpaper`，把本目錄 artifacts 一併帶入。
2. 新設定欄位預設「不接管」：既有使用者升級後桌布不變，需自行在設定中選主題。
3. 資料層新鍵向下相容；使用者需以系統管理員重跑一次 `setup.bat` 取得 15:00 排程（未重跑時天際線延到 18:00 那輪更新）。
4. 回退：設定選「不接管」或執行 `fc-host.exe --restore-wallpaper` 即還原；資料層新鍵可留存不影響舊讀者。

## Open Questions

以下由實作前的探針（tasks 第 1 組）回答，結果原則上只影響 D2／D4／D5 內的分支選擇。例外：1.1 證實 explorer 會累積
GDI，使用者 2026-10-02 決定新增 D11 與規格「explorer 資源安全閥」（tasks 1.7、4.9）。

原列的 ③④⑥ 與焦點讀回已全部解答（見下方各「已解答」節），目前沒有未解答項目。

### 已解答（tasks 1.1／1.2，2026-10-02 連跑）

- **① 反覆設桌布是否使 explorer 累積資源**：有。GDI 與 USER 物件累積且冷卻不回落，增量以每次設定為單位、間歇
  出現；記憶體、CPU 無累積。數據、外推與對策見 Risks 第一條與 D11；證據 `host/tools/evidence/dw-1.1-full.csv`、
  `dw-1.1-full.log`。觸發條件（是否與使用者操作有關）與是否飽和待 tasks 1.7 受控重測。
- **⑤ 設桌布是否廣播訊息**：不會。1,440 次設定加上還原，隱藏頂層視窗 0 則訊息，對照廣播可收到；見 Risks 第二條，
  證據 `host/tools/evidence/dw-1.2-full.log`。淡入閃爍的目視結果另記。
- **附帶發現（供 D5 與 1.3）**：`IDesktopWallpaper::SetWallpaper` 會把 `HKCU\Control Panel\Desktop\Wallpaper` 改成
  `%APPDATA%\Microsoft\Windows\Themes\TranscodedWallpaper`，連設回原 JPG 也一樣；逐螢幕 `GetWallpaper` 則讀回原始路徑。
  讓位判定不得以該登錄值為來源路徑；還原時要連該登錄值一起寫回。證據同 `dw-1.1-full.log` 的 RESTORE-REG 與
  READBACK 行。

### 已解答（tasks 1.6，2026-10-02 查證）

- **Windows 焦點偵測**：官方沒有保證的偵測方式。`IDesktopWallpaper` 的 16 個方法都不涉及焦點，`GetStatus` 只有投影片
  旗標；Policy CSP `Experience/AllowSpotlightCollection` 與 GPO `DisableSpotlightCollectionOnDesktop` 只管「允不允許」，
  不反映目前是否使用。採用非官方的 `DesktopSpotlight\Settings\EnabledState`（見 D5）。本機（build 26200）停用焦點
  後，該機碼與 `Creatives` 仍殘留，焦點圖存放在 `MicrosoftWindows.Client.CBS_*\LocalCache\Microsoft\IrisService\`，
  不是舊的 `ContentDeliveryManager`。出處：learn.microsoft.com 的 `IDesktopWallpaper`／`GetStatus` 參考頁、
  `policy-csp-experience`。
- **Windows 備份**：會備份桌布。support.microsoft.com 的「Windows Backup Settings Catalog」Windows 11 個人化列有
  「Personalize your background」與「Choose a fit for your desktop image」；「Back up and restore with Windows Backup」
  寫明個人化需登入 OneDrive，還原發生在新裝置設定時；中文 UI 名稱「設定 > 帳戶 > Windows 備份」「記住我的喜好設定」
  取自同頁 zh-tw 版。Windows 10 build 20226 曾關閉舊「同步您的設定」的佈景主題同步
  （Windows Insider 部落格 2020-09-30），與現行 Windows 備份是兩回事。對策見 Risks（規格「提示 Windows 備份會帶走
  桌布」）。這是 1.6 唯一新增的規格項，原本 Risks 已預告「若會同步，在設定視窗提示」。
- **CME E-mini S&P 500（ES）**：Globex 週日 18:00 至週五 17:00 ET，每日 17:00–18:00 ET 維護休市（cmegroup.com
  E-mini S&P 500 Contract Specs，2026-10-01 現行頁）。ES 在現貨盤中同樣交易，星盤只畫「現貨休市時段的期貨段」：
  18:00→次日 09:30（Sun–Thu），並補上 16:00–17:00（Mon–Fri）；標籤不寫成官方夜盤。
- **LSE SETS**：開盤競價 07:50–08:00、連續交易 08:00–16:30、收盤競價 16:30–16:35，以上與樣稿相符。收盤後另有
  Closing Price Crossing（CPX）16:35 至約 16:40，以收盤價撮合，要補畫。Pre-Trading 05:05–07:50 與 Post Close
  16:40–17:15 不撮合，不畫：MIT201 §4.4 所列的交易日組成只有競價、連續交易與 CPX。出處：LSE「Millennium Exchange
  and TRADEcho Business Parameters」v10.0 的 Trading Cycles 分頁，以及 MIT201 Issue 15.9 §4.4–4.5，兩者皆自
  2026-07-27 生效。提早收盤日（聖誕前夕、年底最後交易日）連續交易到 12:30，由休市表的半日市欄位涵蓋。
- **ICE FTSE 100 Index Future**：倫敦 01:00–21:00（ice.com 契約規格頁），樣稿「現貨前 01:00–07:50／現貨後
  16:35–21:00」的切法成立。ICE 頁提到的美英夏令錯開週只影響以美國時間表示的時段，星盤以 Europe/London 定義，
  不受影響。

### 已解答（tasks 1.3 於 2026-10-04、1.5 於 2026-10-06，使用者在場）

- **③ PNG 設為桌布後 `GetWallpaper` 讀回什麼**：原路徑，不是轉存路徑。1.1 附帶發現已見逐螢幕讀回為原始路徑，1.3
  設 PNG 後讀回同樣是來源路徑；6.1 B2／B6 也從未讀回快取檔（見 D5「讓位判定」）。出處：commit `8bbb31a`、
  `dw-1.1-full.log` 的 READBACK 行。
- **④ 插拔後螢幕識別是否穩定；`GetMonitorRECT` 是否為實體像素**：穩定、是實體像素。路徑字串與穩定顯示器識別逐字
  相同，縮放、解析度變更、兩次拔插前後都不變；RECT 不受行程 DPI 感知與縮放影響。數據與證據檔見 D4。
- **⑥ explorer 重啟後桌布是否保留、是否需要重設**：保留。1.3 讓 explorer 重啟兩次，讀回仍是原設定的來源路徑；
  宿主沒有誤讓位，並重建 COM 執行緒、系統匣與安全閥基準。出處：commit `8bbb31a`。
- **原桌布為 Windows 焦點時讀回什麼**：`GetWallpaper` 回 `IrisService` 快取的 jpg、`GetStatus`＝1、`EnabledState`＝1，
  切回圖片後 `EnabledState`＝0。`EnabledState` 是 HKCU 下的單一值，沒有逐螢幕的鍵；焦點能否只套用在某一台螢幕未測。
  出處：commit `8bbb31a`。
