# finance-calendar — 財經日曆桌面小工具（fc-host）

> **本檔是專案設定的唯一來源**（2026-09-27 起）。`CLAUDE.md` 只以 `@AGENTS.md` 匯入本檔，所有專案規則改在這裡，勿在 `CLAUDE.md` 另寫內容。

深色半透明的桌面財經儀表板。**現行產品＝`host/` 桌面宿主 `fc-host`（Rust＋Tauri 2，v0.1.1 起）**：常駐系統匣，
把儀表板拆成各自獨立置底的 WebView2 小工具視窗，以 NSIS 安裝檔發佈、內建資料抓取與自動更新。本 repo＝唯一來源，
回覆一律繁體中文。

> **沿革**：Wallpaper Engine（ARM64 上 CEF 以 x64 模擬、反覆 `0xc0000005` 崩潰）→ v5.5 起 Lively Wallpaper
> （2026-07-22/23）→ v0.1.1 起 `host/` 桌面宿主。換掉 Lively 的原因：Lively 把 WebView2 視窗 `SetParent` 進 explorer
> 的 WorkerW，桌布行程被 Restart Manager 終止或卡住時，explorer 會因跨行程同步呼叫逾時被 Windows 重啟、桌布隨之消失
> （memory：`lively-explorer-crash-chain.md`）。**Lively 版已凍結**（使用者決定）：`finance-calendar.html`、
> `update_tw_events.py`、`setup.bat` 不再發版、不加功能，只當搬移來源與資料層行為等價測試的對照基準（oracle）；
> 資料層修正只進 Rust 版。Lively 時代的細節集中在文末「舊版（Lively，已凍結）」。

## 現況（v0.1.1）

- **小工具**：時鐘、總經日曆、台股固定事件、台股動態事件、行情條，加五個保留擴充插槽 `custom1`–`custom5`；每個一個
  獨立頂層視窗，可在設定視窗各自開關。架構見下節「桌面小工具宿主」。
- **版面**：每個顯示器的工作區切成 48×48 格線記錄位置與大小；從系統匣「編輯版面」拖曳移動、調整大小，放開對齊格線，
  重疊或小於最小格數就彈回（design.md D7）。字級由小工具寬度決定（內容倍率＝邏輯寬÷設計寬度，夾 0.5–3）。
- **外觀**：半透明純色背景＋圓角，透明度與主題色可調。**不是毛玻璃**：探針 1.2 證實小工具失焦（常態）時系統背景材質
  不呈現，毛玻璃選項在設定視窗標示為不可用（design.md D8；`desktop.rs` 的 `resolve_appearance`）。
- **捲動**：清單原生捲動，滑鼠滾輪已實機驗證（task 4.7 證據 `host/tools/evidence/4.7-wheel-*.log`）；觸控板兩指捲動
  由使用者實測可捲（2026-10-01、10-04，無腳本證據）；觸控捲動不宣稱——本機沒有觸控螢幕，9/29 的合成觸控拖曳 FAIL（`4.7-touch-summary.log`）是
  腳本 bug（PowerShell 巢狀 struct 賦值只改到複本，送出全 0 的觸控 frame），不是有效結果。
- **資料**：宿主內建 Rust 抓取（`host/src/fetch/`，change `data-layer-rust`），輸出 `tw_events.json` 格式與 Python 版
  相同；來源與規則見「資料來源細節」一節。使用者不必裝 Python、不必建排程。
- **系統匣**：編輯版面、設定、暫停／繼續、版本號、結束（task 5.1）；找到新版時另出現「更新到 vX.Y.Z 並重新啟動」（見「自動更新」）。
- **安裝與更新**：NSIS 安裝檔（使用者層級、不需系統管理員）＋`tauri-plugin-updater` 自動更新；見「安裝」「自動更新」
  「發布」三節。
- **動態桌布（可選）**：預設「不接管」；見「動態桌布」一節。

## 桌面小工具宿主（host/，2026-09-28 起）

`host/`（Rust crate `fc-host`、Tauri 2）是現行產品（v0.1.1 起取代 Lively）：常駐系統匣，把儀表板拆成
可各自開關的小工具視窗。細節依據＝`openspec/changes/archive/2026-10-08-desktop-widget-host/design.md`（D1–D12）；
規格＝同目錄 `specs/`；本節只記跨 task 都要知道、且不會隨進度過期的架構事實。

- **一小工具一視窗**：時鐘、總經日曆、台股固定事件、台股動態事件、行情條，加五個保留擴充插槽
  `custom1`–`custom5`，各自一個獨立頂層 WebView2 視窗、共用同一個 WebView2 環境；**不**
  `SetParent` 進 explorer 的視窗樹，z-order 手動維持在所有一般視窗之下、桌面之上。
- **Win32 集中在 `host/src/desktop.rs`**，其餘模組不得直接呼叫 Win32 API。tao 的
  `always_on_bottom` 類旗標只攔截**之後**的 z-order 變更，視窗**建立當下不會置底**——視窗工廠
  必須在建立後自行 `SetWindowPos(HWND_BOTTOM, ...)` 一次完成顯示與置底（memory：
  `bottom-window-initial-zorder.md`）。
- **核心↔頁面只走「通道快照查詢＋事件推送」**（design.md D4）：資料本體一律經依 webview label
  決定訂閱者的 `tauri::ipc::Channel` 推送，不用 Tauri 事件——`emit_filter`／`emit_to` 只比對
  監聽者自報的 target，裸 `event.listen` 全收、繞得過過濾（memory：
  `tauri-event-target-any-bypasses-filter.md`）。settings／pause／edit-mode 才用事件廣播。
- **資料流**：宿主內建抓取（`host/src/fetch/`，change `data-layer-rust`）把輸出寫到
  `<data_dir>/tw_events.json`，格式與 Python 版相同；`host/src/data.rs` 的 `JsonFileSource` 每 30
  秒比對修改時間＋檔案大小輪詢，有變才讀。**使用者不必另外跑 Python 或排程**。資料目錄＝設定
  `data_dir`（預設 `%LOCALAPPDATA%\tw.fintools.fc-host\data`；**若指向 Python 版輸出夾，例如本機
  `D:\finance-calendar`，要先把 `data_fetch` 設為 `"off"`**，否則宿主預設會抓取並覆寫 Python 產物）。
  抓取排程：台北時間每日 00／06／12／15／18 點（到點後 0–5 分鐘隨機）＋啟動或睡眠恢復後補抓＋到點與
  補抓的兩輪開始間隔下限 60 分鐘（首抓與失敗重試不受此限）；設定 `data_fetch`＝`auto`（預設）／`on`／`off`，
  `auto` 在**隔離偵測**成立時不抓取（環境變數 `LOCALAPPDATA` 與系統登記的不同＝驗收腳本以暫存資料夾
  隔離宿主，避免測試誤打實網、誤改資料），`on` 強制抓取；`--fetch-once <目錄>` 單次抓取後結束
  （不看 `data_fetch`、不經 single-instance，結束碼 0＝寫檔成功／1＝失敗，見 `host/README.md`
  「資料抓取」）。**行為等價測試**：`host/tests/fixtures/fetch/` 情境＋`tests/fetch_oracle/`（Python
  oracle）；改來源時先錄樣本、跑 `expect.py` 再改 Rust。
- **擴充插槽接法**（design.md D5/D6）：接新資料源只動兩處——① `host/src/data.rs` 的通道註冊表
  （沿用某個 `customN` 通道，或加一行新通道＋對應 `DataSource` 實作）；② 把
  `host/ui/widgets/customN.js` 換成實際顯示模組。核心、視窗管理、設定結構不需修改。小工具清單
  有三份：`host/ui/registry.js` 只列 id、訂閱通道與預設開關（顯示模組依 id 載入
  `host/ui/widgets/<id>.js`）；版面欄位只在 Rust——`widgets.rs` 的 `WIDGET_SPECS`（id→通道、
  設計寬度、設計最小高度）與 `settings.rs` 的 `WIDGET_IDS`／`DEFAULT_GRID_RECTS`（預設開關與
  48×48 格線上的預設格座標）。三份清單的 id 集合互相一致，一致性測試只比 id 與通道（並斷言
  registry.js 沒有版面欄位），改一份要核對另外兩份。
- **建置**：純 cargo，不用 tauri-cli 與 Node（`cd host && cargo build --release`，產物
  `host/target/release/fc-host.exe`）。`target/release/` 下的 exe 可能是帶 `--features
  self-test-ipc` 的建置（多出 `self_test_*` 驗收指令）；需要正式行為時先不帶 feature 重建。
  只改 `host/ui/**` 不動任何 `.rs` 時 cargo 不會重新嵌入前端，實機驗收前先 `cargo clean
  --release -p fc-host && cargo build --release`。**安裝檔與更新簽章**一律走
  `host/tools/package.ps1`（流程見「發布」一節與 `host/README.md`），不手動跑 `cargo tauri build`。
- **品質 gate**：`host/` 內 `cargo fmt --check`、`cargo clippy --all-targets`（另跑一次
  `--features self-test-ipc`）、`cargo test`、`cargo test --features self-test-ipc`（self-test 判定與
  事件覆蓋守門測試只在這個 feature 下編譯）；改過的 `.md` 在 repo 根目錄跑
  `npx markdownlint-cli2 <檔>`。寫／審 Rust 前先用 Skill 工具載入 `rust-engineer`。
  改過 `docs/index.html` 時在 repo 根目錄跑 `node --test "tests/landing/*.test.mjs"`（引號 glob 需 Node 21+，更舊版本去掉引號讓 shell 展開；著陸頁 ARM64 偵測；release.yml 的
  version job 也會跑）。
- **桌面行為驗收**：z-order／置底／explorer 重啟復原等用 `host/tools/watch-zorder.ps1` 產生的
  記錄檔佐證，欄位定義與各 `verify-*.ps1` 用法見 `host/tools/README.md`。任何鍵盤／滑鼠／拖曳
  注入一律經 `host/tools/lib/SafeInput.psm1`（唯一入口）；工作階段鎖定或合成輸入未被系統計入時
  會自行停止並回報 BLOCKED／ENV-BLOCKED，不得繞過或重試。前端行為對照測試（Lively 版 vs 新版）
  用 `node host/tests/compare/compare.mjs`（零安裝，靠 Node 22 內建 CDP／http，見
  `host/tests/compare/README.md`）。
- **Rust 開發常見坑**（機制記在 Claude Code 專案 memory：`~/.claude/projects/<本專案>/memory/`，
  索引 `MEMORY.md`；memory 不在 repo 內，其他機器／Codex 看不到——本節所有「memory：」皆指此處）：
  `examples`／`tests` 缺 comctl32 v6 manifest 會載入期卡死（`tauri-manifest-only-linked-to-bins.md`）；
  Tauri event target=Any 繞過過濾（`tauri-event-target-any-bypasses-filter.md`）；置底旗標建立
  當下不生效（`bottom-window-initial-zorder.md`）。

## 自動更新（host/，2026-10 起）

宿主以 `tauri-plugin-updater` 自行檢查、下載、驗簽並安裝新版（安裝檔＝NSIS，安裝與發版流程見「發布」一節）。細節依據＝
`openspec/changes/archive/2026-10-08-installer-auto-update/design.md`（D1–D8）與同目錄 `specs/`；本節只記不會隨進度過期的架構事實。

- **模組地圖**（`host/src/`）：`updater.rs`＝組裝與給 `main.rs`／`tray.rs` 的薄接點；`updater/` 下 `gate`（啟用判定：
  占位公鑰、`dangerous*` 旗標、壞設定一律停用；**隔離環境也停用**——與資料抓取 `auto` 共用 `desktop::detect_isolated_local_app_data`
  （環境變數 `LOCALAPPDATA` 與系統登記的不同＝驗收腳本的暫存資料夾），因安裝檔與 HKCU Run 不跟環境變數走，隔離的舊版宿主
  會把正式版裝進真正的使用者資料夾；`update-e2e` 建置例外，任何環境都能更新）、`schedule`（啟動方式→首次檢查排程）、`marker`（當機標記）、
  `transition`（過渡期原子狀態機）、`engine`（決策核心，後端與副作用皆注入）、`install`（手動／自動／過渡期安裝規則、
  防循環退避）、`exit`（「因更新結束」收尾的時間預算、`StartGate`、`EXIT_OWNER`、安裝失敗後的重新啟動）、
  `plugin_backend`（外掛包裝與 `on_before_exit` 掛鉤）、`state_file`、`version`、`cancel`、`temp_cleanup`。Win32 部分在
  `host/src/desktop/wait_exit.rs`。外掛 capability 不開給任何 webview，更新完全由後端主導。
- **`--wait-exit <pid>`**：`main()` 在啟動仲裁**之前**處理——先以映像路徑與建立時間確認該 PID 是舊宿主，是才等它結束
  （有上限），之後照常仲裁。只用在 `install()` 於 `on_before_exit` 之後失敗時，舊行程以 `--autostart --wait-exit <自己的
  PID>` 重新啟動自己再立刻 `std::process::exit`（不用 `app.exit`，它會走系統匣「結束」的還原路徑）。
- **檔案**（都在 `%LOCALAPPDATA%\tw.fintools.fc-host\`，與 `logs\` 同層）：`startup-marker`＝當機迴圈保護標記，**只由
  Primary 讀寫**，啟動時寫入、存活數分鐘後刪除，正常結束路徑（系統匣結束、過渡期結束、事件迴圈結束、因更新結束）都刪，
  工作階段結束時暫停（取消時恢復；過渡期內不處理，見 design.md D3 已知限制）；啟動時讀到舊標記＝上次非正常結束，進入
  「過渡期」（先檢查更新、有新版就在建立小工具之前安裝）。`update-state.json`＝每次呼叫 `install()` 前（自動、手動、
  過渡期）寫入「嘗試的版本與時間」；只有自動與過渡期受退避限制——同一版本在退避期內只通知、不再自動安裝，防止安裝循環。
- **不變式（違反就是 bug）**：
  - **背景元件先取 `StartGate` 許可**：`build_ui` 裡新增會寫檔、呼叫桌布 API 或啟動執行緒的背景元件，啟動點必須先取
    `updater::try_begin_start` 的許可（取不到＝正在因更新結束，放棄啟動），並持有到 managed state 登記完才放掉；否則
    `--autostart` 後很快自動安裝時，它會在收尾期間啟動、被 `exit(0)` 中途終止。
  - **新的視窗建立或重建入口都要查 `updater::is_exiting_for_update()`**，為真就不建（記 info、不是錯誤）；否則收尾銷毀
    視窗後又被建回來。
  - **系統匣「結束」在任何收尾之前先搶 `EXIT_OWNER`**（`updater::claim_tray_quit`），搶輸就忽略這次「結束」；更新端在
    `on_before_exit` 掛鉤搶同一個擁有權。兩條結束流程互斥，桌布不會在更新時被還原。
  - **安裝前不得自己設 `EXITING_FOR_UPDATE`**：旗標只由 `on_before_exit` 裡的 `prepare_exit_for_update` 設，一旦為真就
    不會復原；`install()` 在 `on_before_exit` 之前失敗時宿主要照常運作。
- **品質 gate** 另加 `--features update-e2e` 的 clippy 與 `cargo test`（這個 feature 放行 e2e 設定檔的 http 端點，並讓更新器
  **略過隔離環境停用**——e2e 要在任何環境都能跑更新；`updater::evaluate_gate` 以 `cfg!` 傳給 `gate::decide`，除此之外別無
  其他作用；正式建置不開）。

## 動態桌布（host/，2026-10 起）

定時以隱藏 WebView2 把主題頁面渲染成 PNG，再以 COM `IDesktopWallpaper` 逐螢幕設成系統桌布；**不**開置底
全螢幕視窗、**不**進 WorkerW。細節依據＝`openspec/changes/archive/2026-10-08-dynamic-wallpaper/design.md`（D1–D12）與同目錄
`specs/`；本節同樣只記不會隨進度過期的架構事實。

- **模組地圖**（`host/src/`）：`wallpaper.rs`＝純函式排程器（`next_action`，含安全閥判定）；`wallpaper_state.rs`
  ＝狀態檔與接管狀態機（讓位、還原、原圖備份）；`wallpaper_render.rs`＝渲染（每次開一個隱藏視窗，回報或 30 秒
  逾時即關）；`wallpaper_coordinator.rs`＝協調迴圈（專屬執行緒，單一計時器＋事件喚醒）；`wallpaper_cli.rs`＝
  `--restore-wallpaper`；`wallpaper_config.rs`＝主題設定檔；`wallpaper_settings.rs`＝設定視窗指令與系統匣通知
  文字；`wallpaper_dnd.rs`＝勿打擾；`app_icon.rs`＝圖示隨主題切換；`webview_env.rs`＝兩組 WebView2 資料夾。
  Win32／COM 在 `host/src/desktop/` 子模組（`wallpaper.rs`＝專屬 COM 執行緒，逾時→忙碌，explorer 卡住不拖累
  呼叫端；另有 `dnd`、`explorer_gdi`、`handoff`、`timezone`、`tray_balloon`）。
- **頁面**：`host/ui/wallpapers/` 五套主題頁（＝`settings.rs` 的 `WallpaperTheme`）＋共用 `lib/`。內建預設
  `config/wallpaper-config.default.json` 以 `include_str!` 編進執行檔、是唯一來源；Rust 只合併頂層鍵，巢狀內容
  以頁面為準。頁面回報的 render id 不符一律丟棄（逾時後才到的 PNG 不算給下一次）。
- **檔案位置**：`wallpaper-config.json`、`wallpaper-state.json` 與 `settings.json` 同在
  `%APPDATA%\tw.fintools.fc-host\`；輸出圖 `%LOCALAPPDATA%\tw.fintools.fc-host\wallpaper\<螢幕鍵>-a.png`／`-b.png`
  兩格交替（6.1 實機：同路徑重設其實會重新載入，交替無害、保留），原桌布備份在其下 `original\`；渲染視窗的 WebView2 資料夾
  `%LOCALAPPDATA%\tw.fintools.fc-host\wallpaper-renderer`；記錄檔同宿主 `%LOCALAPPDATA%\tw.fintools.fc-host\logs`。
- **還原指令** `fc-host.exe --restore-wallpaper`（不建視窗）：沒有宿主在跑就在本行程還原，有就經 single-instance
  交接給它的協調迴圈、以具名事件回報；兩條路都先把主題存成「不接管」。須以**接管過桌布的使用者本人、不提權**
  執行。總時限 90 秒（`CLI_TOTAL_LIMIT`）；**安裝檔至少等 120 秒（`INSTALLER_WAIT_RECOMMENDED`）、逾時不要強制
  結束**——中途被終止只能等宿主下次執行續做，解除安裝不會有下次。結束碼照抄 `wallpaper_cli.rs` 模組檔頭（表外
  任何值都是失敗，例如 101＝還原工作執行緒以外的 panic）：

| 碼 | 意義 |
| --- | --- |
| 0 | 全部還原，或沒有需要還原的桌布（含找不到狀態檔——記錄會寫出實際讀的路徑與使用者） |
| 1 | 還原失敗或狀態檔暫時讀不到（狀態檔保留，可重試）；或桌布已還原但主題設定存檔失敗 |
| 2 | 狀態檔讀不懂（封鎖），沒有動任何東西 |
| 3 | 交給執行中的宿主後時限內沒有回報（宿主可能之後仍會完成） |
| 4 | 有宿主在執行卻交接不了，時限內也沒有結束 |
| 5 | 本行程內部錯誤（無法建立桌布 COM 執行緒或還原執行緒；還原工作執行緒 panic——記錄檔有 `fc_host::panic` 一行） |
| 6 | 已還原所有在線螢幕、仍有離線（或讀不到）的螢幕待還原；安裝檔可視為完成並提示（宿主之後再執行、螢幕接上時才會補做） |
| 7 | 超過命令列總時限 `CLI_TOTAL_LIMIT`（本行程還原做到一半時，狀態檔已有「還原進行中」標記） |

- **不變式（違反就是 bug）**：
  - 資料本體只走 `wallpaper` 通道（`data.rs` 的 `WALLPAPER_CHANNEL`＝`tw-events` 擷取 `WALLPAPER_DATA_KEYS`＋主題
    設定），只給 `wallpaper-renderer-<render id>` 渲染視窗：一律以 `wallpaper_render::is_renderer_label` 判斷
    （每次渲染一個新 label），閘門是 `widgets.rs` 的 `may_access_channel`；不用事件廣播（理由同上節）。
  - **不得**自行補送 `WM_SETTINGCHANGE`，還原也**不得**改用帶 `SPIF_SENDCHANGE` 的 `SystemParametersInfo`
    （design.md Risks：實測 `SetWallpaper` 對頂層視窗零廣播，自行廣播等於把排除掉的風險加回來）；登錄是直接
    寫回 `HKCU\Control Panel\Desktop` 的原始位元組。
  - **狀態檔先寫後設**：設任何桌布前，原桌布與登錄快照必須已寫進狀態檔（`write_durable`＝暫存檔＋`sync_all`
    ＋`rename`）；設定一律經 `WallpaperTakeover::set_monitor_wallpaper`，**不得**直接呼叫
    `WallpaperService::set_wallpaper`。讀不懂＝封鎖：不覆寫、不接管、不還原，另存 `.corrupt-<Unix 秒>` 副本。
  - **原圖備份唯一擁有者**：`original\` 每個備份檔只屬於一筆螢幕紀錄（檔名以螢幕鍵起頭），絕不覆寫、也絕不
    讓螢幕顯示別台的備份。**備份不到原圖就不接管**：原圖複製不了時，接管開始改備份 explorer 的轉存檔，依**設定方式**
    選來源、**不得**以格式或尺寸判斷（6.1 實機：轉存檔的格式與尺寸沿用原圖）。設定方式只看第一次設定**之前**的
    登錄 `Wallpaper` 快照（之後 explorer 會改寫它）：等於各在線螢幕讀回的共同路徑（＝「全部螢幕」設定，
    `SetWallpaper(NULL, 圖)` 把登錄寫成原圖路徑）→ 用全域 `TranscodedWallpaper`，殘留的逐螢幕檔是舊圖、不用；是 Themes 的
    `TranscodedWallpaper`（＝逐螢幕設定）→ 用該螢幕的 `%APPDATA%\Microsoft\Windows\Themes\Transcoded_<索引三位數>`
    （索引＝第一次設定前列舉的 `GetMonitorDevicePathAt` 索引），不存在就不能備份；判斷不出就不能備份。選中的檔要存在、
    大小 > 0、依檔頭認得出是圖片。仍不行（或接管中新接上的螢幕）就不設定、主題改「不接管」並通知。接管中新螢幕的讀回本身就是轉存檔時記為原桌布未知、不備份（內容已是宿主的圖）。備份的
    副檔名：來源有圖片副檔名就沿用，否則依檔頭判斷（PNG／JPEG／BMP／GIF／WebP），判斷不出來用 `.jpg`（轉存檔判斷不出來＝不能備份）。讓位或還原時被取代的舊備份
    （`superseded_backups`）保留到下一次接管開始才清。
  - **讓位以逐螢幕讀回判定**（`judge_readback`）：設定前以 `GetWallpaper` 讀回，既非上次所設、也不在宿主輸出
    資料夾內＝使用者自換，讓位並改「不接管」。**不可**用登錄 `Wallpaper`（`SetWallpaper` 後變成
    `TranscodedWallpaper`）；`S_OK` 也只代表請求被接受。讀回是 explorer 的桌布快取工作檔
    （`StatePaths::is_themes_cache`：Roaming `Themes\` 下的 `TranscodedWallpaper`、同層 `Transcoded_*`、
    `CachedFiles\`、`TranscodedWallpaperCache\` 內的檔）＝無法判定：不讓位、不改原桌布紀錄，下次再判；還原時
    也不算使用者自選，原路徑是快取工作檔就優先用備份。使用者套用的佈景主題
    （`%LOCALAPPDATA%\Microsoft\Windows\Themes\<名稱>\DesktopBackground\`）**不是**快取，照常讓位——不得把判定
    放寬成「路徑含 `\Themes\`」。
  - **離線判定以 `GetMonitorRECT` 失敗為準，不以列舉數量**（2026-10-06 實機）：拔掉的螢幕仍在 `IDesktopWallpaper`
    列舉裡、RECT 回 `E_FAIL`（不是文件寫的 `S_FALSE`）。列舉逐台處理（`assemble_monitor_list`），一台離線不得讓整批
    失敗；但**不在線的項目（`S_FALSE` 或失敗 HRESULT）只有在「在線台數 ≥ 系統作用中顯示器數（`SM_CMONITORS`）」時
    才算離線**，否則（作用中螢幕的暫時錯誤、插回後 explorer 落後於顯示變更事件，落後時兩種回法都可能）整批回錯、
    退避重試——不得把作用中的螢幕當離線（還原會把它標成待還原、命令列回 6，解除安裝後永久停在宿主的圖）。沒有不在線
    項目時不看台數（explorer 漏列某台不得造成永久失敗）。列舉失敗而沿用上次清單時不得以舊座標出圖設定，並依
    `retry_delay` 退避再列舉（沒有要重畫的螢幕時也一樣）；待還原螢幕處理失敗同樣退避重試（之後不一定還有顯示變更
    事件）。explorer 有回應但設定或讀回回錯記 `FailureKind::ApplyError`（退避重試），只有逾時才等下一個時點。
  - **純色是全域狀態**（6.1 實機）：逐螢幕 `SetWallpaper(<螢幕>, "")` 不會變純色（換回 explorer 記憶中的圖片、
    仍回 `S_OK`），只有 `SetWallpaper(NULL, "")`（`WallpaperService::set_solid_color`）才是。原桌布全為空字串時
    狀態檔記 `original.solid_color`；還原純色**不得**用逐螢幕空字串，沒有使用者自選的螢幕時一律走全域純色再寫回
    背景色、填滿方式、登錄。接管途中**還沒設定過**的螢幕被 explorer 換回記憶圖片（`Readback::SolidTransition`；
    設定過但沒生效、讀回仍是設定前記下的同一張也算；設定**確定失敗**時 `last_set` 退回設定前的值＝視為沒設定過，
    逾時不退）不算使用者自換；已設定的螢幕讀回換成別的圖照常讓位。原桌布為純色的待還原螢幕重新接上時不得單獨設
    空字串：其餘在線螢幕仍是純色才再呼叫一次全域純色，否則不動它、清掉待還原，並以 `PendingReport::left_showing_host`
    另列（仍顯示宿主的圖、不再自動還原，記錄要寫明需使用者自行更換）。Windows 焦點啟用中（`EnabledState`＝1）時讀回空字串不算純色。
  - **原桌布是「全部螢幕」設定**（`OriginalDesktop::all_monitors`，以接管前登錄快照推斷，判準同備份轉存檔的選擇）→
    以一次 `SetWallpaper(NULL, 原圖)`（`WallpaperService::set_wallpaper_all`）還原，**不得改成逐螢幕**：宿主逐螢幕設定
    後 `GetWallpaper(NULL)` 變空字串，逐螢幕還原畫面相同卻不會讓它回來（不是原狀態）。條件全成立才走這條
    （`restore_all_monitors_path`：沒有使用者自選、沒有離線螢幕、每台在線螢幕都是要還原或已是這張圖、還原路徑彼此
    相同），否則維持逐螢幕；讓位與待還原一律逐螢幕。
  - **渲染視窗用獨立 WebView2 資料夾**（自己的 browser 行程，關窗即結束）。`WEBVIEW2_USER_DATA_FOLDER` 會蓋掉
    每個視窗指定的資料夾，故 `main()` 開頭由 `webview_env::capture_user_data_override` 讀出並移除，小工具那組改經
    `webview_env::with_widget_data_dir` 指定；新增 webview 建立點也要走這條（memory：
    `webview2-udf-env-overrides-per-window-data-dir.md`）。
  - **下一次渲染前先處置前一個渲染 browser 行程**（`Renderer::settle_previous_browser`，在建立視窗前、不算進 30 秒
    期限）：同一個資料夾的新環境在舊 browser 結束前建立會併進它（關窗後約 100 ms 內）或與它的結束流程重疊，且它偶爾
    卡在結束流程、永不退出（6.2 長跑）。等它結束最多 `BROWSER_EXIT_WAIT`（5 秒），仍在且核對符合（宿主的子行程、
    沒有 `--type=`、`--user-data-dir`＝渲染資料夾的 `EBWebView`）才結束它並記 warn；系統匣「結束」時同樣處置一次；
    其他經 `app.exit` 的結束在 `RunEvent::Exit` 不等待地再處置一次（`settle_on_exit`），工作階段結束中則交給系統。
    PID 一律在 webview 還在時讀、立刻開把手（`desktop::webview_browser_process`），不得事後以 PID 或名稱找行程。
  - 渲染與桌布 COM 請求都會等待，**不得在主執行緒呼叫**（回 `WrongThread`；COM 請求在 debug 建置直接 assert），
    Tauri async 指令也不要直接呼叫（防護攔不到，改經協調迴圈或 `spawn_blocking`）。呼叫 `render()` 時不持有
    任何宿主的鎖（渲染要靠主執行緒建立視窗，持鎖會死結）。
  - **explorer GDI 安全閥**：協調迴圈在確定要渲染時讀一次（`desktop/explorer_gdi.rs`，核心查詢、不送訊息給
    explorer）；比基準增量**超過** 2,000 或絕對值**超過** 8,000（`EXPLORER_GDI_DELTA_LIMIT`／
    `EXPLORER_GDI_ABSOLUTE_LIMIT`）就停止接管、還原、改「不接管」並通知；讀取失敗只記錄、不停止。基準
    （狀態檔 `explorer_baseline`）在接管開始、explorer PID 改變、使用者重新開啟接管時重設。
  - **勿打擾**：官方 WinRT `FocusSessionManager` 與**非官方** WNF `WNF_SHEL_QUIETHOURS_ACTIVE_PROFILE_CHANGED`
    （`ntdll!NtQueryWnfStateData`，`GetProcAddress` 動態取得）OR 合併；讀不到＝未知、**不**暫停。WNF 被 Windows
    更新改掉時退化成只靠官方半邊（最壞是勿打擾時照常重畫）；讀值過期先重讀，偵測故障不會讓桌布永遠不畫。
  - **「不接管」不做新的接管，但協調迴圈照跑**：主題預設「不接管」（`WallpaperTheme::None`），此時不渲染、不讀
    explorer GDI、不查勿打擾（不建輪詢執行緒，已建的停止輪詢）、不設新桌布、圖示用統一圖示。例外只有收尾性的
    還原，都會列舉螢幕、呼叫桌布 COM：狀態檔仍是「接管中」（例如切到「不接管」時工作階段結束、宿主當機）就
    照常還原；帶「還原進行中」標記（例如 `--restore-wallpaper` 中途被終止）就續做；「待還原」螢幕在啟動與顯示
    變更時補還原。所以 Primary 不論主題都要啟動協調迴圈，否則殘留的接管狀態永遠不會還原、桌布停在宿主的圖。
  - **系統匣「結束」一律存成「不接管」**：不論還原是否在時限內完成（沒完成就靠「還原進行中」標記讓下次啟動
    續做）。**唯一例外是 UI 建好之前的「結束」**：更新過渡期，以及沒有舊標記的一般啟動在 setup 同步建立 UI 期間
    （過渡狀態機的「建立 UI」，建立小工具時 WebView2 的巢狀訊息泵可能派送選單事件）——比照過渡期的「結束」，不還原
    桌布、不存主題，刪標記後結束（`tray.rs` 的 `QuitAction::ExitWithoutRestore`；建立小工具途中轉到「結束」就不再
    啟動協調迴圈），狀態檔與主題維持原樣、下次啟動照常，語意同工作階段結束（見
    `openspec/changes/archive/2026-10-08-installer-auto-update/design.md` D3）。把主題改「不接管」的存檔（讓位、安全閥、焦點取消、備份失敗、系統匣結束）失敗時，協調迴圈依
    `retry_delay` 重試到成功，否則重啟後會照設定檔的舊主題重新接管、蓋掉使用者的桌布。
- **self-test 與驗收工具**：`--self-test-render`、`FC_HOST_SELF_TEST_EXPLORER_GDI_FILE`、`FC_HOST_SELF_TEST_DND_FILE`
  只在 `--features self-test-ipc` 建置；連同 `make-icons.mjs`、`wallpaper-shots.mjs`，用法見 `host/tools/README.md`
  （`measure-dw-4.5.ps1`、explorer GDI 注入、`make-icons.mjs`、`wallpaper-shots.mjs` 各節）；勿打擾注入見
  `host/src/wallpaper_dnd/injection.rs` 模組文件。
- **本線的 Rust 開發坑**（memory 位置同上節）：`setwallpaper-side-effects.md`（explorer GDI／USER 間歇累積）；
  `host-one-time-private-memory-step.md`（宿主私有記憶體一次性單步跳升，不是洩漏斜率）。

## 資料來源細節（Rust 版與 Python 版共用）

宿主的 Rust 抓取（`host/src/fetch/`）與凍結的 Python 版 `update_tw_events.py` 走同一套來源、解析規則、韌性設計與輸出格式；
Python 版是行為等價測試的 oracle（`tests/fetch_oracle/`、`host/tests/fixtures/fetch/`，見「桌面小工具宿主」一節的資料流）。
**刻意不同之處**列在 `openspec/changes/archive/2026-10-08-data-layer-rust/design.md` D8，主要是：Rust 不保留「憑證驗證失敗改不驗證」的退回
（嚴格 TLS，SChannel，D2）、上一份輸出中型別不對的鍵逐筆丟棄、不寫 `tw_events.js`、不鏡像 Lively；排程改由宿主依台北時間
固定時點執行（見「桌面小工具宿主」）。以下來源事實兩版皆適用，標「Python 版」者只描述舊版行為。

- 輸出 `tw_events.json`（Python 版另寫 `tw_events.js`；兩者 .gitignore 已排除）的各鍵來源。下文的函式／常數名
  （`fetch_conference_news`、`prev_close_from_chart()`、`fetch_sofr()`、`QUOTES`）是 Python 版識別字；Rust 版對應
  `host/src/fetch/` 的 `conference.rs`、`quotes.rs`（`prev_close_from_chart`、`QUOTES`＝Yahoo 12 檔）、`sofr.rs`，
  「預設 13 檔」＝Yahoo 12 檔＋SOFR：
  - 總經＝ForexFactory 週曆 JSON（thisweek＋nextweek；nextweek 週末才發布、平日自動略過不報錯；USD/EUR/JPY、中高重要性、轉台北時區、內建約 80 條指標中譯字典）
  - **財報／法說會＝MOPS 法人說明會一覽表**（v6.0 起）`POST https://mopsov.twse.com.tw/mops/web/ajax_t100sb02_1`（form：`TYPEK=sii&year=<民國年>&month=<MM>`，抓本月＋下月）→ HTML 表格 regex 解析。**台灣沒有官方「財報公布日」預告 API**（TWSE openapi 143 端點全掃無此物；Yahoo `quoteSummary/calendarEvents` 回 401 需 crumb），法說會就是實務上的財報公布日。只收**市值前百大**；擇要訊息驗得出季別（`第X季`／`XQ26`／`QX`）＝`earnings`、note「Q2 財報」，驗不出＝`conference`、note「法說會」。日期欄有單日 `115/07/16` 與**區間 `115/06/30 至 115/07/03`** 兩種格式（105 筆中 8 筆是區間），一律取起日。上櫃 `TYPEK=otc` 不抓——實測前百大無一家上櫃。備援＝原 openapi t187ap04_L 篩「第12款」（`fetch_conference_news`，只有當日公告、抓不到未來場次）；`top100` 為空視為來源失敗（否則「只收前百大」會靜默產出空清單、被誤讀成「近兩週沒有財報」）
  - **市值前百大**＝`t187ap03_L` 已發行普通股數 × `STOCK_DAY_ALL` 收盤價，取前 `TOP_N`＝100。兩檔合計約 1.6 MB，故**每日只算一次**（快取在輸出的 `top100`／`top100_date`，同日直接沿用不發請求）。抓失敗沿用舊名單，且**必須保留舊的 `top100_date`**，否則會被當成今天已算過而整天不再重試
  - 除權息＝TWSE TWT48U 即時 API（備援 openapi TWT48U_ALL）；股東會＝openapi t187ap41_L；處置股＝上市 openapi `announcement/punish`＋上櫃 TPEx `tpex_disposal_information`（輸出鍵 `punish`；期間雙格式解析（全形～含斜線／半形~無斜線、民國年）、濾權證只留股票/ETF；**二度處置時來源會「第一次＋第二次處置」兩筆公告並存**，同 code 只保留 end 最大那筆，times 取保留筆的 NumberOfAnnouncement＝累計次數；**TPEx openapi 不吃日期參數、空資料回「單筆空白樣板列」而非空陣列**——2026-07-11 實測）
  - 行情＝Yahoo v8 chart（免金鑰；UA 換掉 Python 預設值即可、免 crumb；現價＝`meta.regularMarketPrice`，v8 沒有 `previousClose` 這個鍵）。**昨收絕不可用 `meta.chartPreviousClose`**：那是「所請求 range **起點之前**」的收盤，`range=5d` 下＝數個交易日前，漲跌幅會變成**多日累計**——平靜的一週看起來像日變動，故從 v1 起一直沒被發現（2026-07-31 抓到：加權當日 +7.98% 顯示成 −1.18%、台積電 +9.98% 顯示成 +3.19%）。**改由 `prev_close_from_chart()` 從 close 序列自取**：盤中 Yahoo 會在「當日 K」之外**另附一筆即時報價**（USDTWD=X 實測：當日 K 的 close 為 null、另有一筆盤中即時值），同一交易日佔兩個位置，故先用 `meta.gmtoffset` 換算交易所當地日期歸戶去重（各檔 K 皆落在當地開盤整點、離換日點夠遠），再取最後一個「與當前交易日不同日」的收盤；無 timestamp 可對齊才退回倒數第二筆有效收盤，序列不足才退回 `chartPreviousClose`。`range=5d` 維持不變（要留窗口給連假／停市）。標的在檔頂 `QUOTES`，**預設 13 檔**：USD/TWD、US 10Y、WTI、加權、台積電、日經225、KOSPI、歐股50、道瓊、S&P 500、NASDAQ、**費半（^SOX）**（以上 Yahoo）＋SOFR 3M（NY Fed `sofrai/last/2` 90 天複合平均，`fetch_sofr()` 另抓、append 在最後）。櫃買指數 Yahoo 已停更，要加走 TPEx openapi `/tpex_index`。stooq 備援已死（JS PoW 反爬）。
  - 休市日曆＝TWSE openapi holidaySchedule（民國年 7 碼、**只回當年度**，跨年由前端週末規則兜底）→ 輸出鍵 `holidays`；**排除「開始交易日／最後交易日」說明列**（該 API 混入這類交易日列，如 2026-01-02、02-11、02-23，不得當休市）：`Name` 含 `(開始|最後)交易` 且 `Description` **第一句**（以「。」切）不含「無交易」才排除、不綁日期；「市場無交易，僅辦理結算交割作業」是真休市、一律保留，無論「無交易」寫在 `Name`（2023 起）或 `Description`（2021／2022 年結算日的 Name 也叫「農曆春節前最後交易日」）；只看第一句是因為 2023-01-17（交易日）的說明第二句提到別天無交易
  - **動態桌布三鍵**（供 `dynamic-wallpaper` 桌布主題讀取；Lively 版頁面不讀、既有鍵格式不變；樣本、參數查證與出處見 `tests/fixtures/README.md`，測試 `uv run --no-project python -m unittest discover -s tests -v`）。三鍵各自獨立失敗沿用上次輸出（含其日期）、錯誤記到獨立的 `wallpaper_errors`（字串清單、格式同 `errors`，永遠存在、無錯誤時為 `[]`）——**不進 `errors`**（Lively 頁腳以 `errors` 筆數顯示「⚠ N 個來源異常」）；三鍵**不參與 `fetched` 的判斷**（只有原有來源抓到新資料才刷新，否則只要 Yahoo 通就會蓋掉「已 N 天未更新」警告）；舊輸出裡新鍵型別不對一律當沒有舊值；全新安裝又失敗則省略該鍵。時間判斷一律台北時區（固定 +08:00），不用系統時區：
    - `twii_intraday`＝`{"date":"YYYY-MM-DD","points":[[epoch秒,價],…]}`。Yahoo `^TWII` `interval=5m&range=5d`（`range=5d`＝最近 5 個**交易日**、5m 只提供 60 天內），只收「最後一個完整交易日」＝台北 13:30 已過的最新交易日；盤中執行時當日未收盤序列不覆蓋（取前一個完整日）。依時間排序、相鄰 ≤5 分鐘；只收落在 5 分鐘格線上的點，Yahoo 另附的即時報價（時間離格線）剔除；起點晚於 09:05、終點早於 13:25（來源尚未追上）或有缺口＝視為不完整、當失敗沿用。序列末筆是 13:25 那根，**不含 13:30 收盤試撮**，與官方收盤價不同。
    - `twii_daily`＝`[{"date","open","high","low","close"},…]`，依日期由舊到新，≥40 根（`interval=1d&range=3mo`，實測約 63 根）；只收已收盤日（台北 13:30 前執行會排除當日未收盤那根），OHLC 有 null 的列丟棄、同日重複取後者。
    - `margin`＝`{"date","margin_lots","short_lots","short_margin_ratio","maintenance_ratio","unpriced":[代號…]}`，上市、百分比、取到小數 2 位。TWSE `rwd/zh/marginTrading/MI_MARGN`（`selectType=MS` 取最新已公布的彙總得交易日 D；`ALL&date=D` 取個股；單位：張、融資金額仟元；用「今日餘額」欄）。券資比＝融券張÷融資張×100；融資維持率＝Σ(個股融資張×1000×收盤價)÷(融資金額仟元×1000)×100，查無收盤價者略過（列在 `unpriced`）、ETF 計入。收盤價取 openapi `STOCK_DAY_ALL`（**只有最新交易日、沒有日期參數**），`Date` 須等於 D：不等（15:00 那輪價格已當日、融資晚間才公布）＝不混用，沿用舊值且不算錯誤；D 與舊值同日＝不再下載大表。個股融資張數加總與彙總差 >1% 當解析失敗（防欄位位移）。
  - 韌性（v4.4）：啟動先等網路（socket 探測 `www.twse.com.tw:443`，每 20s、上限 300s，逾時放行）；各來源失敗沿用上次輸出（macro 整鍵、events 三子源按 type、punish 按 market 分段、quotes 逐檔按 name、SOFR、holidays），counts 合併後重算、errors 照實累積；原子寫檔（.tmp→os.replace）；輸出 `fetched` 欄位＝最後一次真的抓到新資料的時間（全來源失敗沿用舊值），`updated` 仍每輪刷新驅動前端重繪。已知取捨：單一來源解析中途壞一筆會整類改用舊快照（不保留部分結果）。
  - **Python 版** HTTPS 取用（`http_json`；Rust 版不採用此退回，見上）：先正常驗證、遇 SSL 憑證問題退回「不驗證」`CERT_NONE` 重試。**坑（2026-07-23 修）**：`urlopen` 會把握手層的 `ssl.SSLCertVerificationError` 包成 `urllib.error.URLError`，故 `except` 必須**同時接 `URLError`**（原本只接 `ssl.SSLError` 會漏接、退回被跳過、整片來源當失敗沿用舊資料）。觸發情境：**較新的 OpenSSL（Python 3.13 內建）對 twse 憑證鏈 AKI/SKI 檢查更嚴 → 全 `twse.com.tw` 來源驗證失敗**（ForexFactory/Yahoo/TPEx 不受影響）；Py3.11/OpenSSL 3.0.13 正常。修法在 `except` 內用 `isinstance(reason, ssl.SSLError)` 判斷、只對 SSL 問題退回，HTTPError（如總經 404）照原樣往上丟。amd64/py3.13 實測、badssl.com 復現驗證。

## 環境限制（重要）

- **目標平台**：Windows 10／11，x64 與 ARM64 各一個安裝檔（ARM64 以 `aarch64-pc-windows-msvc` 交叉編譯，見「發布」）。
  Windows 10 可執行但只有純色外觀、不另外測試（design.md Non-Goals）。ARM64 電腦裝 x64 版會一直以模擬執行，自動更新
  也會一直更新 x64 版（根目錄 `README.md`）。
- **開發機與 ARM64**：本機是 x64、雙 GPU、雙螢幕，與早期文件記載的 ARM64 機器不同（memory：
  `dev-machine-env-differs-from-claude-md.md`）；本機沒有 ARM64 實機，ARM64 以 CI 冒煙為準。
- TradingView widget／iframe 在瀏覽器引擎會崩成死圖——**勿再嘗試**，總經維持自繪。
- 不使用未公開 API（例如 `SetWindowCompositionAttribute`）實作毛玻璃（design.md Non-Goals）。

## 安裝（NSIS 安裝檔）

- 設定在 `host/tauri.conf.json` 的 `bundle`：`targets: ["nsis"]`、`installMode: "currentUser"`（裝在使用者資料夾、不需系統
  管理員）、WebView2 缺少時以 `downloadBootstrapper` 下載安裝；安裝檔 hook 在 `host/installer/hooks.nsh`（UTF-8 含 BOM）。
- **開機自啟登錄歸安裝檔**（使用者決定；memory：`autostart-registration-belongs-to-installer.md`）：只在首次安裝時寫 HKCU
  Run 值、解除安裝時移除；宿主首次啟動不寫，只在使用者切換設定開關時寫（`--autostart` 參數見 `host/README.md`「執行」）。
- **解除安裝先還原桌布**：hook 呼叫 `fc-host.exe --restore-wallpaper`，結束碼處置見「動態桌布」一節與 `hooks.nsh` 檔頭註解。
- 使用者面的安裝、未簽章（SmartScreen／Smart App Control）、覆蓋安裝與解除安裝說明以根目錄 `README.md` 為準；打包一律走
  `host/tools/package.ps1`（見 `host/README.md`「打包與安裝檔」）。

## 發布（GitHub）

### 宿主安裝檔發版（現行，installer-auto-update）

設計＝`openspec/changes/archive/2026-10-08-installer-auto-update/design.md`（D5 金鑰、D6 打包與 CI、D8 ARM64 與檔名）；規格＝同目錄
`specs/app-distribution/spec.md`；打包腳本、CI 與維護者待辦的細節＝`host/README.md`「打包與安裝檔」「CI 發版流程」。

- **產物**（每個 release 必備）：`finance-calendar-setup.exe`（x64）、`finance-calendar-setup-arm64.exe`、兩者各自的 `.sig`、
  `latest.json`。宿主的更新端點＝`releases/latest/download/latest.json`（取「Latest release」那一版的資產）。
- **首發紀錄**：宿主首發版是 **v0.1.1**（2026-10-08）。`v0.1.0` tag 因 `ci-smoke.ps1` 讀取宿主寫入中的記錄檔共用衝突、
  兩架構冒煙皆 FAIL，沒有建立 release；tag 受 ruleset 保護無法移動，修正後升版重發。Lively 的 v5.5–v6.2 已改為 prerelease
  （保留不刪）。實測確認：`release-verify` 的 `release: released` 事件由 draft 發佈觸發；publish 需核准 Environment `release`。
- **`workflow_dispatch` 只在 workflow 檔位於預設分支時可觸發**（GitHub 文件：「This event will only trigger a workflow run
  if the workflow file exists on the default branch.」，
  <https://docs.github.com/en/actions/writing-workflows/choosing-when-your-workflow-runs/events-that-trigger-workflows#workflow_dispatch>）。
  `release.yml` 的 `waive-arm64-smoke` 豁免與 `release-verify.yml` 的手動執行，都要以預設分支上的 workflow 檔觸發。
- **流程**：`host/Cargo.toml` 版本＝tag 版本 → 推 `v*` tag → `.github/workflows/release.yml`（version → build x64／ARM64 交叉
  編譯 → 兩架構冒煙 → publish：Environment `release` 審核後簽章、組 `latest.json`、建立 **draft** release）→ **人工依發佈清單
  檢查** → `gh release edit <tag> --draft=false --latest` → `release-verify.yml`（`release: released` 觸發）從最新 release 下載
  安裝檔驗簽、比對版本，失敗開 issue。不用 `tauri-action`。
- **發佈清單**（發佈 draft 前逐項確認）：① Environment `release` 的審核已通過（簽章私鑰與密碼只放該 Environment，不放 repo 層級
  secrets）；② **x64 實機安裝並啟動**（CI 冒煙只驗行程存活與記錄檔，視窗是否真的出現靠這一項）；③ ARM64 冒煙已通過，
  或 release 說明標有「ARM64 未冒煙」（`waive-arm64-smoke` 豁免，由人工決定是否發佈；本機沒有 ARM64 實機，ARM64 以 CI
  為準）；④ draft 的資產齊全（上列五個），`latest.json` 同時含 `windows-x86_64` 與 `windows-aarch64`；⑤ 發佈指令明寫
  `--latest`（沒指定時 GitHub 依 semver 自動指派 Latest；網頁發佈要手動勾 Set as the latest release）。
- **規則**：**不得發佈不含 `latest.json` 的 release**——Latest 一旦被它搶走，所有使用者的更新器都拿不到更新（`release-verify`
  也會失敗）。**Lively 線不再發 release**（不再上傳 `finance-calendar.zip`）；舊 Lively release 保留、不刪，已改成
  prerelease（見上）。補發修正版一律走同一流程（版本號只升不降，更新器不會降版）；重跑 publish 前先刪同 tag 的殘留 draft。
- **金鑰**（design D5）：簽章私鑰在 repo 外，首發（v0.1.1）後公鑰已寫死在使用者的程式裡，**私鑰遺失就再也推不出更新**；換鑰與
  外洩處置＝用舊私鑰簽「內建新公鑰」的過渡版，使用者更新到過渡版後才改用新私鑰。正式私鑰不用來簽任何測試建置（e2e 用
  `%TEMP%` 的拋棄式金鑰）。
- **未簽章 Authenticode**：SmartScreen／Smart App Control 的影響與使用者說明在根目錄 `README.md`；首發後申請 SignPath
  Foundation，屆時另開 change（目前 job 切分沒有現成插入點，見 design D6）。
- **著陸頁**：`docs/index.html` 的主按鈕與頁尾「下載最新版」已指向 `releases/latest/download/finance-calendar-setup.exe`
  （9cc4dc5）。ARM64 架構偵測（design D8）：以
  `navigator.userAgentData.getHighEntropyValues(['architecture'])` 偵測，ARM 就把主按鈕換成 `-arm64.exe`，並保留兩個架構的
  文字連結；取不到時退回 x64。

## 待辦 / Backlog

- **顯示器關閉／開啟補驗**（使用者 2026-10-06 決定發版後補、2026-10-08 確認改列待辦）：小工具自動暫停（desktop-widget-host 5.5）
  與動態桌布暫停／補畫（dynamic-wallpaper 6.1）都未在最終建置上驗過顯示器關閉。本機一關螢幕就鎖定＋待機（memory
  `monitor-off-locks-and-sleeps-unattended.md`），須使用者在場解鎖；同場可補省電模式實機（需拔電源）。
- **動態桌布在場清單剩項**（不在任何 task 條文內，歸檔時移來）：設定視窗外觀目視（焦點確認對話、等待資料中、休市表提示、
  備份提示）、B3「原桌布為全部螢幕設定且原圖已刪，還原後下次登入是否黑底」、Store 佈景主題（B5）、投影片桌布（B8）、
  explorer 卡住時系統匣是否阻塞、安全閥通知橫幅目視、設定視窗對「等待重試」狀態無提示（要不要加由使用者決定）。
- **v0.1.1 首發審查延後的 low**：冒煙未實際斷言首裝寫 Run／解除安裝刪 Run（第 8 步讀 Run 有競態）、uninstall 中途 Abort
  只能等 300 秒逾時；DW 協調迴圈在要求待處理時仍跑一次唯讀列舉、顯示變更分支在 `session_ending` 中仍補還原待還原螢幕；
  桌面捷徑改名 hook 的更新模式遷移未測、`desk-cancel` 測試無鑑別力、Rename 失敗無痕跡。
- 資料層「網路不重連」另案：家用 WiFi 連不到來源時資料停舊、換熱點才更新——屬環境/可達性，非腳本 bug；可加「各來源成敗＋可達性」診斷記錄。
- 資料層（只做 Rust 版，Python 版凍結）：加 TPEx 上櫃除權息／櫃買指數（`/tpex_index`）；興櫃處置 `tpex_esb_disposal_information`；注意股票類型（TWSE/TPEx 均有端點）；台股固定事件遇假日順延標示。
- **宿主：字型大小可由使用者設定**（使用者 2026-10-06 提出，v0.1.1 之後開 change）：現況字級完全由小工具寬度決定
  （內容倍率＝矩形邏輯寬 ÷ 設計寬度、夾 0.5–3，見 `openspec/changes/archive/2026-10-08-desktop-widget-host/design.md` D7；Lively 版的
  `uiScale` 已在 proposal 移除），想放大字只能把小工具拉寬。開工前要決定：全域一個字級或每個小工具各自設定；以
  「寬度倍率 × 字級倍率」疊加時最小格數要跟著重算，否則放大後內容塞不下；動態桌布主題頁不納入（構圖依螢幕實體像素
  等比，規格明定不受縮放影響）。
- **宿主：編輯版面時顯示格線**（使用者 2026-10-06 提出，v0.1.1 之後開 change）：進入編輯版面時，在各顯示器工作區畫出
  48×48 格線，讓使用者拖曳與調整大小時看得出會對齊到哪、最小／最大能到多大。現況只有對齊與不合法時的紅框預告（D7），
  沒有整片格線。要注意：格線要逐顯示器依該台工作區與 DPI 計算（`layout.rs` 的格線像素函式）；畫格線的視窗必須不搶
  焦點、滑鼠穿透、z-order 在小工具之下（Win32 只能經 `host/src/desktop.rs`），離開編輯版面就關閉；調整大小時可加亮
  該小工具的最小格數範圍。

## 驗證

- **品質 gate**：見「桌面小工具宿主」的品質 gate（`cargo fmt --check`、各 feature 組合的 `cargo clippy --all-targets` 與
  `cargo test`、改過的 `.md` 跑 `npx markdownlint-cli2`）；指令彙整在 `host/README.md`「工具與測試」。
- **桌面行為**：`host/tools/` 的 `watch-zorder.ps1` 與 `verify-*.ps1`（用法、欄位定義、證據檔命名見 `host/tools/README.md`；
  證據在 `host/tools/evidence/`）。注入輸入一律經 `lib/SafeInput.psm1`；**視覺驗收前先確認工作階段未鎖定**（鎖定時截圖
  全黑、注入會打進密碼框，memory：`screen-capture-black-when-locked.md`、`input-injection-while-locked-locks-account.md`）。
- **前端**：`node host/tests/compare/compare.mjs`（Lively 版 vs 新版對照）與 `node host/tests/<檔名>.test.mjs`；見
  `host/README.md`「工具與測試」。
- **資料抓取**：`fc-host.exe --fetch-once <目錄>` 單次實網抓取（結束碼與記錄見 `host/README.md`「資料抓取」）；行為等價測試
  見「桌面小工具宿主」的資料流。
- **執行中的宿主**：記錄檔在 `%LOCALAPPDATA%\tw.fintools.fc-host\logs\`（每日輪替、保留 7 天），設定檔
  `%APPDATA%\tw.fintools.fc-host\settings.json`；完整路徑表見 `host/README.md`「設定檔與記錄檔位置」。
- **headless 截圖**：`msedge --headless=new --disable-gpu --window-size=W,H --virtual-time-budget=6000 --dump-dom（或 --screenshot=路徑）"file:///<絕對路徑>"`。
  注意：virtual-time 下 **smooth scroll 動畫不會跑**；用獨立 `--user-data-dir` 避免附掛到既有 Edge 實例；viewport 比
  視窗小（memory：`headless-edge-viewport-smaller-than-window.md`）。

## 舊版（Lively，已凍結）

v0.1.1 起由宿主取代；以下保留作搬移來源、對照測試（`host/tests/compare/`）與維護舊版時的參考，**不再發版、不加功能**。
資料層 Python 版的來源細節已併入上方「資料來源細節」。

### 頁面（`finance-calendar.html`）

- `finance-calendar.html` 單檔自繪，深色毛玻璃（宿主小工具改為半透明純色）。版面錨定右上（`.layout`＝`position:fixed; top:0; right:0` 內容尺寸容器＋`transform:scale(var(--z))`、origin 右上）：中右＝時鐘＋總經日曆、最右＝台股固定＋動態事件、底部橫貫＝行情條，左側留白給桌面圖示。所有參數在頂部 `CONFIG`。頂部 `const VERSION` 顯示於時鐘卡右下角（確認 Lively 套用成功）。
- **資料流（方案 B，Python 版）**：Lively 會把桌布**整包複製**到自身 Library，且其 WebView2 **擋掉絕對 `file://` 跨資料夾讀取**（實測：複製夾有好資料但桌布仍空）。故：html 的 `CONFIG.dataBase=''`（相對路徑、讀「自身資料夾」那份 `tw_events.js`），資料鮮度由資料層投遞——`update_tw_events.py` 的 `lively_wallpaper_dirs()` **動態尋找** Lively 桌布複製夾（`%LOCALAPPDATA%\Packages\12030rocksdanister.LivelyWallpaper_*\...\Library\**\finance-calendar.html`），併入 `out_dirs`、沿用既有原子寫入一起鏡像過去。不寫死隨機碼、重匯桌布自動跟上；找不到＝靜默略過，`sys.argv[1:]` 為萬用後路。
- **屬性面板（Lively）**：`LivelyProperties.json`（slider×3＋color×1）＋ HTML `window.livelyPropertyListener(name,val)` → 複用與 WE 相同的 `setScale()/--panel-o/--blur/--accent` 邏輯。Lively slider 用 **`tick`（刻度數）不是 step**（`tick=(max−min)/step+1`）、無 `order`（靠 JSON 排列）、color 回傳 `#RRGGBB`。屬性名一律小寫：`uiscale`／`panelopacity`／`blurpx`／`accentcolor`。WE 的 `wallpaperPropertyListener`＋`project.json` 滑桿**保留不動**（雙棲；project.json 已從 repo 取消追蹤但本機留著）。
- 縮放：`setScale()` 設 CSS 變數 `--z`，整體佔用面積等比縮放。**預設 `CONFIG.uiScale=1.0`（100%，v5.5 由 125% 改，小螢幕友善）**。防呆：欄高上限 `calc(min(92vh,92vh/--z)-52px-var(--qh))`；寬度夾限 z≤(innerWidth−8)/約1052px（掛 resize 重算）。
- **捲動（v5.1–5.4，Lively 專屬）**：Lively 的滑鼠轉發**只含「點擊＋移動」、不含滾輪/拖曳**（`Settings.json` `InputForward:1`＋`MouseInputMovAlways:true`，無「開滾輪」選項）。故直欄過長清單改 `setupScrollButtons()`：正上/正下方各放一顆 ▴/▾ **翻頁鈕**（點一下捲近一頁、含淡出漸層避開文字、只在該方向有內容才顯示），**捲到最底時 ▾ 變「回頂鈕」**（上橫線＋▴）。隱藏原生捲軸（`.evlist::-webkit-scrollbar{width:0}`）。`CONFIG.columnAutoScroll:false`（預設按鈕）／`true`（自動來回輪播 `setupAutoScroll()`，給 WE 那種連點擊都收不到的環境）。底部行情條維持水平跑馬燈 `setupAutoScrollX()`。
- 排程與重繪：HTML 每 6h（`reloadHours`）重讀資料＋重算固定事件；**每 1 分鐘輕量重讀**（`freshCheckMin`）——`updated` 有變才重繪＝不打斷輪播；每日 04:00（`dailyReloadAt`）整頁重載讓「本週」視窗前滾。台股固定事件（第三週三台指結算、3/6/9/12 第三週五季結算、財報 3/31・5/15・8/14・11/14、每月 10 日營收截止）純本地計算。**「本週」＝週日–週六**（v6.0 由週一–週日改，`weekStartOf`；全檔僅 `renderFixed` 用它）。
- 凍結偵測心跳（v4.5）：Lively 在**有視窗覆蓋/最大化時會暫停桌布**（同 WE `playbackmaximized:pause`），JS 計時器一起凍結。對策＝每秒 `tickClock()` 兼任心跳：距上次 tick >90 秒（剛解凍）→ `loadData(true)`；跨日 → `refreshAll()` 翻日。`loadData` 防重入（`loadBusyMs` 序列化＋30 秒保險絲）：解凍瞬間心跳與 freshCheck 併發，兩個動態 script 共享 `window.TW_EVENTS`、`script.remove()` 不中止在途請求，極端時序舊資料會蓋新資料。
- 前端資料防護（v4.4）：`renderData` 拒收「合法但全空」payload（`isEmptyPayload`＋非空 `lastGoodData` 時不覆蓋）；頁腳「資料更新」「已 N 天未更新」看 `fetched`（舊檔無此欄退回 `updated`）。
- 底部行情條：`.layout` 第三格 `grid-column:1/-1`、`quotes` 鍵驅動、紅漲綠跌（`--up`/`--dn`）、yield 類顯示絕對值、無資料自動隱藏；超寬時 `setupAutoScrollX()` 頭尾相接連續跑馬燈（`CONFIG.quotesLoop`，false＝來回）；`CONFIG.showQuotes` 開關；以 `--qh` 從兩欄高度預算讓位。
- 台股動態事件上下兩節：**上節「預告清單」**（`dynTypes` 中 punish 以外，預設**財報**・法說會・股東會；`earnings` 排 `dynTypes` 最前，使同日財報排在法說會之前——排序第二鍵是該陣列索引）列窗口內今天起所有場次；**下節「處置股」當日制**（今天；非交易日順延下一交易日、`holidays`＋週末判定、掃描上限 30 天）。節標題 `.dayhead` 插在同一 `#dynList`（保住捲動邏輯）；`window.__TEST_TODAY='YYYY-MM-DD'` 可覆寫今天供測試。除權息照抓、預設不顯示（加回 `'dividend'` 即進上節）；`#dynList` 解除 50vh 上限自然攤開、`#panelFixed{flex:none}` 防擠壓。
- 時區：總經事件由 `ts`（epoch）＋**系統時區**動態換算（人在東京自動 +1h），頁腳標「本機時區（UTC±N）」；台股「今天」跟隨系統日期（過午夜翻日、遇休市順延）；**只有資料層抓取窗口釘 Asia/Taipei**（防排程 00:00 JST 濾掉台北當日事件）；時鐘走系統時區。舊資料（無 ts）退回台北字串。

### Lively 的環境行為

- **Lively 三個關鍵行為**（決定舊版架構）：① 只轉發滑鼠點擊/移動、**無滾輪/拖曳** → 捲動用 ▴▾ 按鈕；② WebView2 **擋絕對 `file://` 跨資料夾讀取** → 資料靠鏡像投遞、桌布相對讀自身夾；③ **複製桌布**到自身 Library（`SaveData\wptmp\<guid>` 或 `wallpapers\<id>`，隨機碼會變）→ 動態尋找、勿寫死。source 放 `LivelyInfo.json` 會讓資料夾匯入報「already packaged」失敗——故 source 不放，靠拖曳單一 html 匯入時 Lively 自生。
- **Python**：不寫死架構/路徑。`setup.bat` 動態偵測（`py -3`→PATH→常見安裝位置），沒有就用 **winget** 自動裝（`Python.Python.3.12`，自動選 arm64/amd64）。純 stdlib 故任何 Python 3.x 皆可跑。

### 安裝與排程（一鍵 `setup.bat`）

- **`setup.bat`（根目錄、純 ASCII/CRLF/無 BOM）**＝自我提權殼（UAC），呼叫 **`scripts/setup.ps1`（UTF-8 含 BOM）**＝全部邏輯。防呆：管理員檢查、腳本存在、偵測/winget 自動裝 Python、偵測/winget msstore 自動裝 **Store 版 Lively**（id `9NTM2QC6QWS7`＝rocksdanister 官方；**勿裝 GitHub 版 `rocksdanister.LivelyWallpaper`**——package 路徑不同、鏡像會找不到）、確認桌布已在 Lively 設好（＝鏡像有目標，沒有只黃警告不中止）、建排程、實跑一次驗鏡像命中、全程 [OK]/[WARN]/[FAIL] 彩色輸出（實跑那步：完成訊息含「警告／失敗」標黃而非綠、逐行「失敗」也標黃、正常的「下週檔 404」維持灰——2026-07-23 修）＋結尾 pause。驗證用 `python.exe`（收得到輸出）、排程用 `pythonw.exe`（不閃視窗）。
- **排程**：任務名 `TW財經桌布資料更新`，用 **`Register-ScheduledTask`（非 schtasks/XML）建**——避開「XML 必須 UTF-16 LE＋BOM 否則中文亂碼」的坑。每日 06:00 起每 6h＋每日 15:00＋登入後 2 分鐘＋`StartWhenAvailable`＋電池可跑＋30 分鐘上限。**身分＝登入使用者（Interactive/Limited）**——關鍵：跑成 SYSTEM 的話 `%LOCALAPPDATA%` 會對到 SYSTEM、找不到使用者的 Lively 夾、鏡像失效。
- 教訓（仍有效）：`/TR` 執行檔必用絕對路徑（排程器不做 PATH 解析）；管理員建立的任務非管理員改不動（Access denied）；`0x800710E0`＝條件拒絕（錯過不補跑／電池被擋）；開機補跑撞網路未就緒（19 來源全敗回傳碼仍 0）→ 靠資料層 v4.4 韌性（等網路＋fallback），勿加 `RunOnlyIfNetworkAvailable`。
- 移除舊版：`uninstall.bat`（刪排程、可選移除 Lively／Python），使用者說明見根目錄 `README.md`「舊版（Lively）已停止維護」。

### Lively 版發布（凍結，不再發版）

- **著陸頁** `docs/index.html`（GitHub Pages，Source＝main `/docs`）：深色＋金色 accent、五套主題左右交錯；主 CTA 與頁尾「下載最新版」→ **`releases/latest/download/finance-calendar.zip`（Lively 時代的做法；現已改指 `finance-calendar-setup.exe`，見上）**（一鍵直接下載 asset、不進 release 頁）；版本號 `#ver` 另抓 `releases/latest` API 顯示（同源）。hero 不放主視覺，由主題段截圖呈現；含 `og-image.png`（1200×630 貼 LINE 用）、`favicon.svg/.png`。
- **發布檔**：`finance-calendar.html`、`update_tw_events.py`、`LivelyProperties.json`、`setup.bat`、`scripts/setup.ps1`、`bg.png`、`README.md`、`AGENTS.md`、`CLAUDE.md`、`docs/`（著陸頁）。
- **背景 `bg.png` 的源**：`assets/bg-source.html`（零依賴獨立 HTML、**本機保留、git 不追蹤、不進 zip**——見 `.gitignore`）＝從 claude.ai/design「股市桌面桌布.dc.html」忠實移植（K 線 `makeCandles` seed 521、像素等效）。重生成＝headless Edge 依螢幕原生解析度截圖成 PNG（指令＋解析度查法寫在該檔頭部註解；2026-07 本機 2880×1920）。PNG 保高解析、勿壓 8-bit（漸層會色帶）。
- **打包發布 zip（Lively 線舊流程；該線已凍結，不再發版，改走上面的宿主流程。當年的做法如下）**：`git archive --format=zip --prefix=finance-calendar/ -o dist/finance-calendar.zip HEAD finance-calendar.html update_tw_events.py LivelyProperties.json setup.bat uninstall.bat bg.png README.md AGENTS.md CLAUDE.md scripts` → `gh release upload <tag> dist/finance-calendar.zip --clobber`。**固定檔名 `finance-calendar.zip`**（`releases/latest/download/` 只認手動上傳的固定名 asset，GitHub 自動 source zip 沒有 latest 別名）；`--prefix` 讓解壓成單層 `finance-calendar/` 夾；只收「可執行產品」組、**不含 `docs/`**（著陸頁走 Pages，下載包不需要）；`dist/` 本機留、git 不追蹤。
- **不發布**（`.gitignore` 排除，本機保留）：`docs/SPEC-*.md`、`tasks/`、`project.json`、`tw_update_task.xml`、`.markdownlint-cli2.jsonc`、`tw_events.js/.json`、`preview.jpg`、`*.bak`、`dist/`（發布 zip 打包夾）、`assets/`（背景 `bg.png` 的渲染源）。
- **版本慣例**：HTML 頂部 `const VERSION`，每批改動遞增。對照：v4.5 `96a8faa`、v5.5（遷移 Lively）`4a0b265`、setup+README `a26c254`、v5.6（SSL 退回修正＋setup 判色，見 tag v5.6）、**v6.0（本週改週日起算＋前百大財報公布日、法說會改接 MOPS）**、**v6.1（行情條漲跌幅修正：昨收改由 close 序列自取，原本是多日累計）**。
- git remote：`https://github.com/Benjamin-Teng/finance-calendar.git`（main 直接 push 同步；GitHub Pages＝main `/docs`、Release v5.5 在 GitHub 端，push 後 Pages 自動重建）。

### Lively 版驗證

- **在 Lively 實機看動畫前，先把該螢幕所有視窗最小化**——Lively 有視窗覆蓋/最大化就暫停桌布（含時鐘凍結），別誤判「效果失效」。
- **改 html 要在 Lively 重新匯入才生效**（Lively 複製檔案）：拖 `finance-calendar.html` 進 Lively。**資料改動**則走鏡像自動更新、不用重匯。重匯後新複製夾（新隨機碼）由 `lively_wallpaper_dirs()` 自動找到。
- **headless 截圖/驗證（大量使用）**：`msedge --headless=new --disable-gpu --window-size=W,H --virtual-time-budget=6000 --dump-dom（或 --screenshot=路徑）"file:///<絕對路徑>"`。注意：virtual-time 下 **smooth scroll 動畫不會跑**（測捲動用即時或看 handler 有無觸發）；用獨立 `--user-data-dir` 避免附掛到既有 Edge 實例。
- 直接用瀏覽器開 `finance-calendar.html` 可預覽（`dataBase=''` 讀同夾 tw_events.js；跨目錄絕對 file:// 在 Edge 可、在 Lively 不可）。`python update_tw_events.py` 會列出各來源筆數＋警告＋「已輸出 →」的鏡像目標。
- 查桌布是否 Lively 在畫、是否原生 ARM64：`Get-Process Lively,msedgewebview2,webwallpaper64`（要有前兩者、無 webwallpaper64）；架構用 `IsWow64Process2`（processMachine=0＝原生）。查崩潰：Windows 應用程式事件記錄 `webwallpaper64`／`msedgewebview2` 的 APPCRASH。

### Lively 時代的待辦（凍結，不再處理）

- **Phase 4 搬家 ✅ 已完成（2026-07-23）**：repo 已從 Steam 夾（`...\wallpaper_engine\projects\myprojects\finance-calendar`）搬到 `C:\projects\finance-calendar`（脫離 Steam），git／remote 不變、程式無寫死舊路徑（`dataBase` 相對、setup 用 `%~dp0/$PSScriptRoot`）；已從新路徑實跑 `update_tw_events.py` 驗證資料鏡像命中 Lively 複製夾。**收尾動作（需使用者本人做，AI 無法提權/刪 Program Files）**：① 以系統管理員重跑 `setup.bat` 用新路徑重建排程（`Register-ScheduledTask` 需管理員；舊機器上該排程原本就不存在，等同全新建立）；② 步驟①完成後，手動刪舊夾 `C:\Program Files (x86)\Steam\...\myprojects\finance-calendar`。
- Phase 3 穩定觀察：連續數日、多次睡眠喚醒，事件記錄無新 `msedgewebview2` 崩潰＝根治確認。
- 心跳升級：改接 Lively `livelyWallpaperPlaybackChanged(IsPaused)` 事件取代計時器猜凍結（需 `LivelyInfo.json` Arguments 加 `--pause-event true`）。
