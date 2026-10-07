# Tasks

> 執行路徑：SDD｜理由：跨 Python 資料層、Rust 核心／Win32、前端繪圖頁三個模組，task 超過 8 個，含 explorer 隔離、
> 使用者桌布還原等生命週期與數值計算（券資比、維持率、時區換算）高風險面，且核心 task 建立在探針結論之上，需要逐 task review。
>
> 開工前提：desktop-widget-host 已合併進 main；從 main 開 `feat/dynamic-wallpaper`。第 1 組探針是拋棄式，可在合併前提前進行，
> 但 1.1–1.5 會改桌布、啟動宿主或增加 explorer 負擔，**必須等 desktop-widget-host 的 6.2 連跑與實機驗收批次結束**
> （連跑期間不得跑任何 probe）；1.6 純查證任何時候可做。
>
> 審查路由：逐 task 審查與最終全分支審查一律由 Opus subagent 執行，不用 Codex（使用者 2026-10-02 指示，優先於全域
> CLAUDE.md 的 Codex 規定）。

所有 Rust task 先載入 `rust-engineer` skill，完成前在 `host/` 內跑 `cargo fmt --check`、`cargo clippy --all-targets`
（另跑一次 `--features self-test-ipc`）、`cargo test` 並附輸出；Python task 跑 `ruff check`、`ty check`；改過的 `.md` 在
repo 根目錄跑 `npx markdownlint-cli2 <檔>`。改到 `host/ui/**` 而沒改 `.rs` 時，實機驗收前先
`cargo clean --release -p fc-host && cargo build --release`。桌面行為驗收附 `host/tools/` 記錄檔，視覺驗收前先確認
工作階段未鎖定；任何輸入注入一律經 `host/tools/lib/SafeInput.psm1`。

## 1. 探針（拋棄式，結論寫回 design.md 的 Open Questions 與對應 Decision）

- [x] 1.1 explorer 負擔探針：拋棄式程式以 `IDesktopWallpaper` 對兩台螢幕每 10 秒交替設兩張 4K PNG、連續 2 小時，每 30 秒記錄 explorer 的 CPU、工作集、控制代碼數、GDI／USER 物件數；探針開始前備份、結束後還原使用者原桌布與填滿方式；交付 CSV 與「是否累積」結論
- [x] 1.2 閃爍與廣播探針：同一探針加一個隱藏頂層視窗計數設桌布時收到的 `WM_SETTINGCHANGE`／`WM_DISPLAYCHANGE` 等訊息；目視記錄是否有淡入閃爍；交付訊息計數記錄與結論
- [x] 1.3 讀回與 explorer 重啟探針：設 PNG 後立即 `GetWallpaper` 讀回並記錄路徑；結束 explorer 讓它重啟後再讀回並截圖；交付記錄，另請使用者切到 Windows 焦點後讀回一次（記錄 `GetWallpaper` 回傳與 `EnabledState`）；結論寫回 D5（讓位判定比對方式）與 Open Questions ⑥
- [x] 1.4 渲染成本探針：在 release build 的宿主內以隱藏 WebView2 載入星盤樣稿畫 3840×2160 並回傳 PNG 位元組，量測「每次開關視窗」與「常駐只重畫」兩種方式的耗時與記憶體各 10 次；結論寫回 D2
- [ ] 1.5 螢幕識別探針（排最後、需使用者在場）：記錄插拔外接螢幕、切換縮放前後的 `GetMonitorDevicePathAt` 值與 `GetMonitorRECT`，並與既有穩定顯示器識別對應；結論寫回 D4
- [x] 1.6 查證三件事並寫回 design.md（附官方出處）：Windows 焦點模式的偵測方式；Windows 備份同步是否會上傳桌布；以瀏覽器目視重驗 CME E-mini S&P 500 與 LSE SETS 的現行時段
- [x] 1.7 GDI 累積受控重測（使用者不在電腦前的時段、需事先約定）：沿用 1.1 探針，只對一台螢幕每 10 秒設定、連續 6 小時、每次設定後 5 秒取樣；CSV 加記閒置秒數（`GetLastInputInfo`，只讀）與 explorer 頂層視窗數；交付「是否飽和、是否與使用者操作有關」結論，回填 Risks 第一條與 D11 門檻

## 2. 資料層（Python，`update_tw_events.py`）

- [x] 2.1 錄製 Yahoo v8 chart（盤中、日線）與 TWSE `MI_MARGN`（`MS`、`ALL`）真實回應為測試樣本，查證並記錄 `interval`／`range` 參數；交付樣本檔與參數出處
- [x] 2.2 新增加權指數盤中走勢鍵：只在台北 13:30 收盤後的交易日才更新，否則保留上一個完整交易日；以樣本測試涵蓋「12:00 那輪不覆蓋」「15:00 那輪更新」「失敗沿用舊值並記錯誤」三種情況
- [x] 2.3 新增日 K 鍵（≥40 個交易日、依日期排序）：以樣本測試筆數、排序與失敗沿用
- [x] 2.4 新增上市券資比與融資維持率鍵（含所屬交易日）：以樣本測試 2026-09-30 算出券資比約 2.53%、查無收盤價個股被略過、晚間公布前沿用前一交易日；Lively 版頁面以含新鍵與不含新鍵的兩份輸出，在固定 `__TEST_TODAY` 下 headless `--dump-dom` 比對（排除時鐘與頁腳更新時間元素）結果一致
- [x] 2.5 `scripts/setup.ps1` 排程加每日 15:00 觸發（原觸發不變、`-Force` 可重跑）；以 `Get-ScheduledTask` 輸出驗證觸發清單（**已由 data-layer-rust 的 15:00 排程時點取代**：該 change 對 `wallpaper-market-data` 的 MODIFIED delta「每日 15:00 更新」，宿主內建抓取已含台北 15:00 時點；Lively 版不再發版，本項不需執行）——**結案（2026-10-06）**：`setup.ps1` 的 15:00 觸發程式已在（第 161 行起）；`Get-ScheduledTask` 實機驗證不執行——要以系統管理員重跑 `setup.bat`，且 Lively 版已凍結、v0.1.0 起資料由宿主內建抓取，排程不再是產品路徑

## 3. 繪圖頁（`host/ui/wallpapers/`）

- [x] 3.1 共用模組與測試工具：尺寸基準、決定性亂數、字型載入（Cinzel、霞鶩文楷 TC、Noto Serif TC 放 `fonts/` 並附 OFL 授權）、通道介面與 `w`/`h`/`t`/`tz`/`fixture` query 參數；截圖指令稿以 headless Edge 輸出五種尺寸（3840×2160、2560×1600、1920×1080、2560×1080、1080×1920）；以離線狀態截圖驗證字型為內建字型
- [x] 3.2 主題設定檔內建預設值（時段表初值取自 `bg-sessions.html` 的 MARKETS、各市場當年度休市表與假日名、旺季表、宜忌詞庫、門檻）：休市表初值逐筆附官方出處；以 JSON schema 驗證
- [x] 3.3 星盤頁（以 `bg-sessions.html` full 構圖為基礎，加休市變暗與「休市（假日名）」）：Node 內建 test runner 單元測試時段展開——美歐夏令切換週、台指期結算日、Overnight 生效日前後、跨日夜盤、休市日、「接下來」不含午休；五種尺寸截圖，並由頁面在 fixture 模式回報文字邊界框、斷言全在畫面內且兩兩不相交
- [x] 3.4 撕日曆頁：「宜」「忌」規則抽成純函式並單元測試 spec 中各情境（休市日、台積電財報＋旺季、無事件取詞庫且同日同句、券資比 1.9%、休市日優先於融資警示、法說會資料缺漏時略過、20 字詞庫句單行縮小字級不超出）；五種尺寸截圖＋文字邊界框斷言
- [x] 3.5 脊線與等高線頁：以盤中走勢資料取代亂數並標示交易日；資料落後 3 個交易日起角落顯示「資料停在 M/D」（落後 2 個不顯示，邊界案例要測）；以樣本 fixture 五種尺寸截圖
- [x] 3.6 天際線頁：20 根日 K＋每根 20MA，以 40 筆樣本驗證第一根 K 的 20MA 正確（與獨立計算比對）；過期標示同 3.5；五種尺寸截圖

## 4. 宿主核心（Rust）

- [x] 4.1 設定與主題設定檔：`Settings` 新增桌布主題（預設「不接管」）；`wallpaper-config.json` 讀取、逐鍵合併內建預設、損壞時警告並退回；單元測試涵蓋缺檔、缺鍵、型別錯、損壞
- [x] 4.2 `desktop.rs` 桌布 COM 執行緒（D3）：列舉螢幕、讀取、設定、讀寫填滿方式／純色／投影片；呼叫端逾時與「忙碌」旗標；以假的阻塞實作做單元測試驗證逾時後不再送新請求、呼叫端不卡住
- [x] 4.3 `wallpaper.rs` 排程純函式（D6）：單元測試涵蓋五套主題的時點、星盤對齊 15 分、暫停中錯過時點於恢復時補畫一次、啟動／換主題／螢幕變更／DPI 變更觸發、首次無資料時「等待資料」
- [x] 4.4 狀態檔與狀態機（D5）：先寫後設、原圖備份、讓位判定、還原（不接管／系統匣結束）與不還原（工作階段結束）、新螢幕先記錄；單元測試涵蓋 spec 中 desktop-wallpaper 的各情境（以假的桌布 API）
- [x] 4.5 渲染管線（D2、D4）：隱藏渲染視窗、PNG 位元組 IPC、每螢幕 a/b 交替寫檔、30 秒逾時與退避；依 1.4 結論每次開關、渲染視窗使用獨立 WebView2 環境（自己的 user data folder），回應帶 render id 比對；實測並回報收尾線三件事：關閉後殘留是一次性或累加、獨立環境下 browser 行程是否結束並歸還記憶體與冷啟動成本、15 分鐘頻率 2 小時記憶體曲線；以假渲染器單元測試 30 秒逾時保留舊圖、連續失敗退避間隔遞增、兩次時點之間無繪製；以 self-test 指令在 release build 產出一張 4K PNG 並驗證尺寸
- [x] 4.6 `wallpaper` 通道（D7）：由 `tw-events` 擷取所需鍵並附主題設定，只推給 label 前綴為 `wallpaper-renderer-` 的渲染視窗（`is_renderer_label`）；沿用 D4 的裸 `event.listen` 負向測試，證明其他小工具收不到
- [x] 4.7 整合接線：暫停原因與 `PauseRules`（一般視窗最大化不暫停）、顯示變更、設定變更、系統匣「結束」還原（含填滿方式）、工作階段結束不還原、當機後重啟照常接管、`--restore-wallpaper` 命令列（不建立小工具視窗）；每條路徑各一份記錄檔，記錄需含觸發事件、排程決策與桌布 API 呼叫結果
- [x] 4.8 設定視窗：主題選單、「等待資料中」、Windows 焦點確認提示（取消時維持不接管）、休市表快過期提示、讓位時系統匣通知；依 1.6 結論實作 Windows 焦點偵測（讀 `DesktopSpotlight\Settings` 的 `EnabledState`，偵測不到時視同一般圖片）、主題選單常駐 Windows 備份提示；以 compare／截圖驗證各狀態
- [x] 4.9 explorer 資源安全閥（D11）：`desktop.rs` 以 `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)`＋`GetGuiResources(GR_GDIOBJECTS)` 讀 explorer（殼層視窗擁有者）的 GDI 物件數；狀態檔記基準值與 explorer PID；`next_action` 依讀值決定停止接管；觸發時還原原桌布與填滿方式、主題改「不接管」、系統匣通知；單元測試涵蓋增量 >2000、絕對值 >8000、PID 改變重設基準、使用者重新開啟時重設基準、讀取失敗時不停止只記錄（以假的讀值）

## 5. 圖示

- [x] 5.1 圖示產製指令稿（headless Edge 點陣化＋Node 封裝 PNG 內嵌 ICO）：統一圖示 C（≥32px 用 `c-sunrise-page.svg`、16／20 用 `c-16.svg`、24 用 `c-24.svg`）與五套主題圖示（各自在 16px 截圖檢驗，辨識不清者補簡化造型）；SVG 搬進 `host/icons/src/`；交付 `.ico` 並以檔頭解析驗證每個檔都含 16、20、24、32、40、48、64、256 各尺寸
- [x] 5.2 exe 內嵌統一圖示、系統匣與設定視窗隨主題即時切換（不接管與讓位時回統一圖示）；`docs/favicon.svg`／`.png` 換成統一圖示；驗證：從 release exe 取出圖示資源比對各尺寸、headless 開著陸頁確認 favicon 連結、系統匣在 100%／150% 縮放下截圖、切換主題後系統匣圖示改變的截圖

## 6. 驗收與收尾

- [ ] 6.1 實機驗收（記錄檔佐證）：雙螢幕各自出圖且解析度正確、拔插螢幕、切換縮放、鎖定後解鎖補畫、強制重啟 explorer、使用者自換桌布讓位＋通知、系統匣結束還原、登出不還原、`--restore-wallpaper` 還原、安全閥觸發（以 self-test 指令注入假的 GDI 讀值，確認還原＋改不接管＋通知）；顯示器關閉相關項排最後並留使用者回座補查。**使用者裁定（2026-10-06）**：顯示器關閉／開啟延到 v0.1.0 發版後補驗（關閉即鎖定，須使用者解鎖）；「插拔後補畫」「切換縮放後重畫」搭收尾線在場驗收第一段（主題星盤）一起驗
- [x] 6.2 長時間觀察：星盤主題連續執行 24 小時，記錄 explorer 資源曲線、宿主輸出圖數量（≤ 螢幕數 × 2）與原桌布備份數量（≤ 螢幕數）
- [x] 6.3 文件：`AGENTS.md` 新增「動態桌布」一節（架構事實、設定檔位置、還原指令）；`docs/handover.md` 更新 active change；markdownlint 通過
- [x] 6.4 整支分支 Codex adversarial-review，findings 實測重現後處理，直到無未決項
