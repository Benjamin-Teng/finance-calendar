# finance-calendar — Lively Wallpaper 財經日曆桌布

深色毛玻璃的桌面財經儀表板。**底層＝ [Lively Wallpaper](https://www.rocksdanister.com/lively/)（原生 ARM64/x64 WebView2）**；本資料夾（`C:\projects\finance-calendar`，2026-07-23 已脫離 Steam）＝唯一來源，回覆一律繁體中文。

> **v5.5 起從 Wallpaper Engine 遷移到 Lively**（2026-07-22/23）：WE 的網頁渲染行程 `webwallpaper64.exe`（libcef.dll）在 ARM64 上是 x64 模擬執行、反覆 `0xc0000005` 崩潰致整張桌布消失需手動重選；Lively 的 `msedgewebview2` 為**原生 ARM64**，結構性消除崩潰。

## 現況（2026-07-23）

- `finance-calendar.html` 單檔自繪，深色毛玻璃。版面錨定右上（`.layout`＝`position:fixed; top:0; right:0` 內容尺寸容器＋`transform:scale(var(--z))`、origin 右上）：中右＝時鐘＋總經日曆、最右＝台股固定＋動態事件、底部橫貫＝行情條，左側留白給桌面圖示。所有參數在頂部 `CONFIG`。頂部 `const VERSION` 顯示於時鐘卡右下角（確認 Lively 套用成功）。
- **資料流（方案 B）**：Lively 會把桌布**整包複製**到自身 Library，且其 WebView2 **擋掉絕對 `file://` 跨資料夾讀取**（實測：複製夾有好資料但桌布仍空）。故：html 的 `CONFIG.dataBase=''`（相對路徑、讀「自身資料夾」那份 `tw_events.js`），資料鮮度由資料層投遞——`update_tw_events.py` 的 `lively_wallpaper_dirs()` **動態尋找** Lively 桌布複製夾（`%LOCALAPPDATA%\Packages\12030rocksdanister.LivelyWallpaper_*\...\Library\**\finance-calendar.html`），併入 `out_dirs`、沿用既有原子寫入一起鏡像過去。不寫死隨機碼、重匯桌布自動跟上；找不到＝靜默略過，`sys.argv[1:]` 為萬用後路。
- 資料層 `update_tw_events.py`（**純 stdlib、零依賴**）→ 產出 `tw_events.js/.json`（.gitignore 已排除）：
  - 總經＝ForexFactory 週曆 JSON（thisweek＋nextweek；nextweek 週末才發布、平日自動略過不報錯；USD/EUR/JPY、中高重要性、轉台北時區、內建約 80 條指標中譯字典）
  - 除權息＝TWSE TWT48U 即時 API（備援 openapi TWT48U_ALL）；股東會＝openapi t187ap41_L；法說會＝openapi t187ap04_L 篩「第12款」；處置股＝上市 openapi `announcement/punish`＋上櫃 TPEx `tpex_disposal_information`（輸出鍵 `punish`；期間雙格式解析（全形～含斜線／半形~無斜線、民國年）、濾權證只留股票/ETF；**二度處置時來源會「第一次＋第二次處置」兩筆公告並存**，同 code 只保留 end 最大那筆，times 取保留筆的 NumberOfAnnouncement＝累計次數；**TPEx openapi 不吃日期參數、空資料回「單筆空白樣板列」而非空陣列**——2026-07-11 實測）
  - 行情＝Yahoo v8 chart（免金鑰；UA 換掉 Python 預設值即可、免 crumb；欄位 `meta.regularMarketPrice`／`chartPreviousClose`——v8 沒有 `previousClose` 這個鍵）。標的在檔頂 `QUOTES`，**預設 13 檔**：USD/TWD、US 10Y、WTI、加權、台積電、日經225、KOSPI、歐股50、道瓊、S&P 500、NASDAQ、**費半（^SOX）**（以上 Yahoo）＋SOFR 3M（NY Fed `sofrai/last/2` 90 天複合平均，`fetch_sofr()` 另抓、append 在最後）。櫃買指數 Yahoo 已停更，要加走 TPEx openapi `/tpex_index`。stooq 備援已死（JS PoW 反爬）。
  - 休市日曆＝TWSE openapi holidaySchedule（民國年 7 碼、**只回當年度**，跨年由前端週末規則兜底）→ 輸出鍵 `holidays`
  - 韌性（v4.4）：啟動先等網路（socket 探測 `www.twse.com.tw:443`，每 20s、上限 300s，逾時放行）；各來源失敗沿用上次輸出（macro 整鍵、events 三子源按 type、punish 按 market 分段、quotes 逐檔按 name、SOFR、holidays），counts 合併後重算、errors 照實累積；原子寫檔（.tmp→os.replace）；輸出 `fetched` 欄位＝最後一次真的抓到新資料的時間（全來源失敗沿用舊值），`updated` 仍每輪刷新驅動前端重繪。已知取捨：單一來源解析中途壞一筆會整類改用舊快照（不保留部分結果）。
- **屬性面板（Lively）**：`LivelyProperties.json`（slider×3＋color×1）＋ HTML `window.livelyPropertyListener(name,val)` → 複用與 WE 相同的 `setScale()/--panel-o/--blur/--accent` 邏輯。Lively slider 用 **`tick`（刻度數）不是 step**（`tick=(max−min)/step+1`）、無 `order`（靠 JSON 排列）、color 回傳 `#RRGGBB`。屬性名一律小寫：`uiscale`／`panelopacity`／`blurpx`／`accentcolor`。WE 的 `wallpaperPropertyListener`＋`project.json` 滑桿**保留不動**（雙棲；project.json 已從 repo 取消追蹤但本機留著）。
- 縮放：`setScale()` 設 CSS 變數 `--z`，整體佔用面積等比縮放。**預設 `CONFIG.uiScale=1.0`（100%，v5.5 由 125% 改，小螢幕友善）**。防呆：欄高上限 `calc(min(92vh,92vh/--z)-52px-var(--qh))`；寬度夾限 z≤(innerWidth−8)/約1052px（掛 resize 重算）。
- **捲動（v5.1–5.4，Lively 專屬）**：Lively 的滑鼠轉發**只含「點擊＋移動」、不含滾輪/拖曳**（`Settings.json` `InputForward:1`＋`MouseInputMovAlways:true`，無「開滾輪」選項）。故直欄過長清單改 `setupScrollButtons()`：正上/正下方各放一顆 ▴/▾ **翻頁鈕**（點一下捲近一頁、含淡出漸層避開文字、只在該方向有內容才顯示），**捲到最底時 ▾ 變「回頂鈕」**（上橫線＋▴）。隱藏原生捲軸（`.evlist::-webkit-scrollbar{width:0}`）。`CONFIG.columnAutoScroll:false`（預設按鈕）／`true`（自動來回輪播 `setupAutoScroll()`，給 WE 那種連點擊都收不到的環境）。底部行情條維持水平跑馬燈 `setupAutoScrollX()`。
- 排程與重繪：HTML 每 6h（`reloadHours`）重讀資料＋重算固定事件；**每 1 分鐘輕量重讀**（`freshCheckMin`）——`updated` 有變才重繪＝不打斷輪播；每日 04:00（`dailyReloadAt`）整頁重載讓「本週」視窗前滾。台股固定事件（第三週三台指結算、3/6/9/12 第三週五季結算、財報 3/31・5/15・8/14・11/14、每月 10 日營收截止）純本地計算。
- 凍結偵測心跳（v4.5）：Lively 在**有視窗覆蓋/最大化時會暫停桌布**（同 WE `playbackmaximized:pause`），JS 計時器一起凍結。對策＝每秒 `tickClock()` 兼任心跳：距上次 tick >90 秒（剛解凍）→ `loadData(true)`；跨日 → `refreshAll()` 翻日。`loadData` 防重入（`loadBusyMs` 序列化＋30 秒保險絲）：解凍瞬間心跳與 freshCheck 併發，兩個動態 script 共享 `window.TW_EVENTS`、`script.remove()` 不中止在途請求，極端時序舊資料會蓋新資料。
- 前端資料防護（v4.4）：`renderData` 拒收「合法但全空」payload（`isEmptyPayload`＋非空 `lastGoodData` 時不覆蓋）；頁腳「資料更新」「已 N 天未更新」看 `fetched`（舊檔無此欄退回 `updated`）。
- 底部行情條：`.layout` 第三格 `grid-column:1/-1`、`quotes` 鍵驅動、紅漲綠跌（`--up`/`--dn`）、yield 類顯示絕對值、無資料自動隱藏；超寬時 `setupAutoScrollX()` 頭尾相接連續跑馬燈（`CONFIG.quotesLoop`，false＝來回）；`CONFIG.showQuotes` 開關；以 `--qh` 從兩欄高度預算讓位。
- 台股動態事件上下兩節：**上節「預告清單」**（`dynTypes` 中 punish 以外，預設法說會・股東會）列窗口內今天起所有場次；**下節「處置股」當日制**（今天；非交易日順延下一交易日、`holidays`＋週末判定、掃描上限 30 天）。節標題 `.dayhead` 插在同一 `#dynList`（保住捲動邏輯）；`window.__TEST_TODAY='YYYY-MM-DD'` 可覆寫今天供測試。除權息照抓、預設不顯示（加回 `'dividend'` 即進上節）；`#dynList` 解除 50vh 上限自然攤開、`#panelFixed{flex:none}` 防擠壓。
- 時區：總經事件由 `ts`（epoch）＋**系統時區**動態換算（人在東京自動 +1h），頁腳標「本機時區（UTC±N）」；台股「今天」跟隨系統日期（過午夜翻日、遇休市順延）；**只有資料層抓取窗口釘 Asia/Taipei**（防排程 00:00 JST 濾掉台北當日事件）；時鐘走系統時區。舊資料（無 ts）退回台北字串。

## 環境限制（重要）

- **ARM64 Windows**。WE 的 CEF（webwallpaper64／libcef.dll）在 ARM 上 x64 模擬、反覆崩潰——已改 Lively（原生 ARM64 WebView2）。TradingView widget／iframe 在瀏覽器引擎會崩成死圖——**勿再嘗試**，總經維持自繪。
- **Lively 三個關鍵行為**（決定架構）：① 只轉發滑鼠點擊/移動、**無滾輪/拖曳** → 捲動用 ▴▾ 按鈕；② WebView2 **擋絕對 `file://` 跨資料夾讀取** → 資料靠鏡像投遞、桌布相對讀自身夾；③ **複製桌布**到自身 Library（`SaveData\wptmp\<guid>` 或 `wallpapers\<id>`，隨機碼會變）→ 動態尋找、勿寫死。source 放 `LivelyInfo.json` 會讓資料夾匯入報「already packaged」失敗——故 source 不放，靠拖曳單一 html 匯入時 Lively 自生。
- **Python**：不寫死架構/路徑。`setup.bat` 動態偵測（`py -3`→PATH→常見安裝位置），沒有就用 **winget** 自動裝（`Python.Python.3.12`，自動選 arm64/amd64）。純 stdlib 故任何 Python 3.x 皆可跑。

## 安裝與排程（一鍵 `setup.bat`）

- **`setup.bat`（根目錄、純 ASCII/CRLF/無 BOM）**＝自我提權殼（UAC），呼叫 **`scripts/setup.ps1`（UTF-8 含 BOM）**＝全部邏輯。防呆：管理員檢查、腳本存在、偵測/winget 自動裝 Python、偵測/winget msstore 自動裝 **Store 版 Lively**（id `9NTM2QC6QWS7`＝rocksdanister 官方；**勿裝 GitHub 版 `rocksdanister.LivelyWallpaper`**——package 路徑不同、鏡像會找不到）、確認桌布已在 Lively 設好（＝鏡像有目標，沒有只黃警告不中止）、建排程、實跑一次驗鏡像命中、全程 [OK]/[WARN]/[FAIL] 彩色輸出＋結尾 pause。驗證用 `python.exe`（收得到輸出）、排程用 `pythonw.exe`（不閃視窗）。
- **排程**：任務名 `TW財經桌布資料更新`，用 **`Register-ScheduledTask`（非 schtasks/XML）建**——避開「XML 必須 UTF-16 LE＋BOM 否則中文亂碼」的坑。每日 06:00 起每 6h＋登入後 2 分鐘＋`StartWhenAvailable`＋電池可跑＋30 分鐘上限。**身分＝登入使用者（Interactive/Limited）**——關鍵：跑成 SYSTEM 的話 `%LOCALAPPDATA%` 會對到 SYSTEM、找不到使用者的 Lively 夾、鏡像失效。
- 教訓（仍有效）：`/TR` 執行檔必用絕對路徑（排程器不做 PATH 解析）；管理員建立的任務非管理員改不動（Access denied）；`0x800710E0`＝條件拒絕（錯過不補跑／電池被擋）；開機補跑撞網路未就緒（19 來源全敗回傳碼仍 0）→ 靠資料層 v4.4 韌性（等網路＋fallback），勿加 `RunOnlyIfNetworkAvailable`。

## 發布（GitHub）

- **著陸頁** `docs/index.html`（GitHub Pages，Source＝main `/docs`）：xs_helper 風格骨架＋本專案金色 accent；主 CTA「下載最新版」→ `releases/latest`（**需先建 Release**）。含 `hero.png`（桌布截圖）、`og-image.png`（1200×630 貼 LINE 用）、`favicon.svg/.png`。
- **發布檔**：`finance-calendar.html`、`update_tw_events.py`、`LivelyProperties.json`、`setup.bat`、`scripts/setup.ps1`、`bg.jpg`、`README.md`、`CLAUDE.md`、`docs/`（著陸頁）。
- **不發布**（`.gitignore` 排除，本機保留）：`docs/SPEC-*.md`、`tasks/`、`project.json`、`tw_update_task.xml`、`.markdownlint-cli2.jsonc`、`tw_events.js/.json`、`preview.jpg`、`*.bak`。
- **版本慣例**：HTML 頂部 `const VERSION`，每批改動遞增。對照：v4.5 `96a8faa`、v5.5（遷移 Lively）`4a0b265`、setup+README `a26c254`。
- git remote：`https://github.com/Benjamin-Teng/finance-calendar.git`（main 直接 push 同步；GitHub Pages＝main `/docs`、Release v5.5 在 GitHub 端，push 後 Pages 自動重建）。

## 待辦 / Backlog

- **Phase 4 搬家 ✅ 已完成（2026-07-23）**：repo 已從 Steam 夾（`...\wallpaper_engine\projects\myprojects\finance-calendar`）搬到 `C:\projects\finance-calendar`（脫離 Steam），git／remote 不變、程式無寫死舊路徑（`dataBase` 相對、setup 用 `%~dp0/$PSScriptRoot`）；已從新路徑實跑 `update_tw_events.py` 驗證資料鏡像命中 Lively 複製夾。**收尾動作（需使用者本人做，AI 無法提權/刪 Program Files）**：① 以系統管理員重跑 `setup.bat` 用新路徑重建排程（`Register-ScheduledTask` 需管理員；舊機器上該排程原本就不存在，等同全新建立）；② 步驟①完成後，手動刪舊夾 `C:\Program Files (x86)\Steam\...\myprojects\finance-calendar`。
- Phase 3 穩定觀察：連續數日、多次睡眠喚醒，事件記錄無新 `msedgewebview2` 崩潰＝根治確認。
- 資料層「網路不重連」另案：家用 WiFi 連不到來源時資料停舊、換熱點才更新——屬環境/可達性，非腳本 bug；可加「各來源成敗＋可達性」診斷記錄。
- 心跳升級：改接 Lively `livelyWallpaperPlaybackChanged(IsPaused)` 事件取代計時器猜凍結（需 `LivelyInfo.json` Arguments 加 `--pause-event true`）。
- 法說會改接 MOPS 即時來源（openapi 重大訊息快取偏舊，未來場次會消失）；加 TPEx 上櫃除權息／櫃買指數（`/tpex_index`）；興櫃處置 `tpex_esb_disposal_information`；注意股票類型（TWSE/TPEx 均有端點）；台股固定事件遇假日順延標示。

## 驗證

- **在 Lively 實機看動畫前，先把該螢幕所有視窗最小化**——Lively 有視窗覆蓋/最大化就暫停桌布（含時鐘凍結），別誤判「效果失效」。
- **改 html 要在 Lively 重新匯入才生效**（Lively 複製檔案）：拖 `finance-calendar.html` 進 Lively。**資料改動**則走鏡像自動更新、不用重匯。重匯後新複製夾（新隨機碼）由 `lively_wallpaper_dirs()` 自動找到。
- **headless 截圖/驗證（大量使用）**：`msedge --headless=new --disable-gpu --window-size=W,H --virtual-time-budget=6000 --dump-dom（或 --screenshot=路徑）"file:///<絕對路徑>"`。注意：virtual-time 下 **smooth scroll 動畫不會跑**（測捲動用即時或看 handler 有無觸發）；用獨立 `--user-data-dir` 避免附掛到既有 Edge 實例。
- 直接用瀏覽器開 `finance-calendar.html` 可預覽（`dataBase=''` 讀同夾 tw_events.js；跨目錄絕對 file:// 在 Edge 可、在 Lively 不可）。`python update_tw_events.py` 會列出各來源筆數＋警告＋「已輸出 →」的鏡像目標。
- 查桌布是否 Lively 在畫、是否原生 ARM64：`Get-Process Lively,msedgewebview2,webwallpaper64`（要有前兩者、無 webwallpaper64）；架構用 `IsWow64Process2`（processMachine=0＝原生）。查崩潰：Windows 應用程式事件記錄 `webwallpaper64`／`msedgewebview2` 的 APPCRASH。
