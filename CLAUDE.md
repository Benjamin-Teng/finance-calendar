# finance-calendar — Wallpaper Engine 財經日曆桌布

金十桌面風格的 WE 網頁桌布。本資料夾（WE 專案）＝唯一來源，回覆一律繁體中文。

## 現況（2026-07-16）

- `finance-calendar.html` 單檔自繪，深色毛玻璃。版面整體錨定右上（`.layout` 為 `position:fixed; top:0; right:0` 內容尺寸容器＋`transform:scale(var(--z))`、origin 右上）：中右＝時鐘＋總經日曆、最右＝台股固定＋動態事件，左側留白給桌面圖示。所有參數在頂部 CONFIG。
- 資料層 `update_tw_events.py`（純 stdlib、零依賴）→ 產出 `tw_events.js/.json`（.gitignore 已排除）：
  - 總經＝ForexFactory 週曆 JSON（thisweek＋nextweek；nextweek 週末才發布、平日自動略過不報錯；USD/EUR/JPY、中高重要性、轉台北時區、內建約 80 條指標中譯字典）
  - 除權息＝TWSE TWT48U 即時 API（備援 openapi TWT48U_ALL）；股東會＝openapi t187ap41_L；法說會＝openapi t187ap04_L 篩「第12款」；處置股＝上市 openapi `announcement/punish`＋上櫃 TPEx `tpex_disposal_information`（輸出鍵 `punish`；期間雙格式解析（全形～含斜線／半形~無斜線、民國年）、濾權證只留股票/ETF；**二度處置時來源會「第一次＋第二次處置」兩筆公告並存**（期間重疊、新令取代舊令，2026-07-11 實測 TWSE），同 code 只保留 end 最大那筆，times 取保留筆的 NumberOfAnnouncement＝累計次數（來源會把同檔所有列該欄位都改成累計值）；**TPEx openapi 不吃日期參數、空資料回「單筆空白樣板列」而非空陣列**——2026-07-11 實測）
  - 行情＝Yahoo v8 chart（免金鑰；UA 換掉 Python 預設值即可、免 crumb；欄位 `meta.regularMarketPrice`／`chartPreviousClose`——v8 沒有 `previousClose` 這個鍵）。標的在檔頂 `QUOTES`，預設 12 檔：USD/TWD、US 10Y、WTI、加權、台積電、日經225、KOSPI、歐股50、道瓊、S&P 500、NASDAQ（以上 Yahoo）＋SOFR 3M（NY Fed `sofrai/last/2` 90 天複合平均，`fetch_sofr()` 另抓、append 在最後；日變動極小，前端捨入為 0.00 時顯示灰色持平）。櫃買指數 Yahoo 已停更，要加走 TPEx openapi `/tpex_index`。stooq 備援已死（全站 JS PoW 反爬，2026-07-10 實測）
  - 休市日曆＝TWSE openapi holidaySchedule（民國年 7 碼、**只回當年度**，跨年由前端週末規則兜底）→ 輸出鍵 `holidays`
  - 韌性（v4.4，2026-07-15）：啟動先等網路（socket 探測 `www.twse.com.tw:443`，每 20s、上限 300s，逾時放行）；各來源失敗沿用上次輸出（macro 整鍵、events 三子源按 type、punish 按 market 分段、quotes 逐檔按 name、SOFR、holidays），counts 合併後重算、errors 照實累積；原子寫檔（.tmp→os.replace）；輸出新增 `fetched` 欄位＝最後一次真的抓到新資料的時間（全來源失敗沿用舊值），`updated` 仍每輪刷新驅動前端重繪。動機：開機補跑在網路未就緒時執行，曾把好資料整份洗成空（2026-07-15 實測 19 來源全敗）。已知取捨：單一來源解析中途壞一筆會整類改用舊快照（不保留部分結果）。
- WE 屬性滑桿（`project.json`，鍵名必須全小寫）：uiscale／panelopacity／blurpx／accentcolor → HTML `wallpaperPropertyListener` 接收。
- 縮放：`setScale()` 設 CSS 變數 `--z`，整體佔用面積等比縮放（縮小往右上收）。防呆：欄高上限 `calc(min(92vh, 92vh/--z) - 52px)`（52＝容器上下 padding）防底部裁切；寬度夾限 z ≤ (innerWidth−8)/約1052px 防窄視窗左緣裁切（掛 resize 重算）。
- 清單超高自動來回輪播 `setupAutoScroll()`（桌布在圖示層下收不到滾輪；2026-07-10 修正端點判斷未含行進方向的死循環——此功能此前從未真正動過）；HTML 每 6h 重讀資料、每日 04:00 reload、**每 1 分鐘輕量重讀**（`freshCheckMin`，v4.5 由 5 改 1——睡眠喚醒時頁面重啟常比排程補跑寫完資料早幾十秒，5 分鐘空窗太長（2026-07-16 實測 20:20 頁面重啟、20:20:56 資料才落地）；`updated` 有變才重繪＝不打斷輪播；配合登入排程，開機後幾分鐘內新資料就上桌）；台股固定事件（第三週三台指結算、3/6/9/12 第三週五季結算、財報 3/31・5/15・8/14・11/14、每月 10 日營收截止）純本地計算。
- 前端資料防護（v4.4）：`renderData` 拒收「合法但全空」payload——`isEmptyPayload`（macro/events/punish/quotes 長度和 0）為真且已有非空 `lastGoodData` 時不覆蓋不重繪（頁面重載後歸零，屬 session 內第三道保險）；頁腳「資料更新」與「已 N 天未更新」警示改看 `fetched`（舊資料檔無此欄退回 `updated`）。
- 凍結偵測心跳（v4.5，2026-07-16）：WE 暫停（`playbackmaximized/fullscreen: pause`）與系統睡眠會把頁面連 JS 計時器一起凍結，freshCheck 的 5 分鐘須累計「未凍結時間」才數得滿——睡眠喚醒後若持續在最大化視窗工作，新資料長期上不了桌（2026-07-16 實測：喚醒後 1 分鐘資料就緒、20 分鐘仍未重繪）。對策＝每秒 `tickClock()` 兼任心跳：距上次 tick >90 秒（剛解凍）→ `loadData(true)`；跨日（含自然過午夜）→ `refreshAll()` 讓「今天」翻日。並補 `loadData` 防重入（`loadBusyMs` 序列化＋30 秒保險絲）：解凍瞬間心跳與恢復的 freshCheck 併發呼叫，兩個動態 script 共享 `window.TW_EVENTS` 且 `script.remove()` 不會中止在途請求，極端時序舊資料會蓋掉新資料。附帶查證：喚醒時 StartWhenAvailable 會補跑睡眠期間錯過的 6h 場次（撞上在途執行個體時記事件 322 略過），資料層喚醒場景免加觸發器；`playbacksleep: stop` 會在喚醒時整頁重啟（webwallpaper64 行程重生），不影響本機制。
- 底部行情條（2026-07-10 新增）：`.layout` 第三格 `grid-column:1/-1`，`quotes` 鍵驅動、紅漲綠跌（`--up`/`--dn`）、yield 類漲跌顯示絕對值、無資料自動隱藏；超寬時 `setupAutoScrollX()` 滾動——預設頭尾相接連續跑馬燈（`CONFIG.quotesLoop`，2026-07-11 新增：確認超寬後把 quotes 列複製第二份、`loopAt`＝兩份首項 offsetLeft 差，捲過一份寬度無縫跳回、不停頓），`quotesLoop:false` 改回來回輪播（端點停 `autoScrollPauseSec` 再反向）；`CONFIG.showQuotes` 開關；顯示時以 root 變數 `--qh` 從兩欄高度預算讓位。
- 台股動態事件上下兩節（2026-07-11 改版，取代原純當日制）：**上節「預告清單」**（`dynTypes` 中 punish 以外的類型，預設法說會・股東會）列資料窗口內今天（含）起的所有場次——這類預先排定、只看目標日幾乎天天空白（改版動機）；列附日期、當天標「今天」badge、note 與 chip 同字（如「法說會」）不重述。**下節「處置股」維持當日制**：目標日（今天；非交易日順延下一交易日，`holidays`＋週末判定、掃描上限 30 天）落在處置期間內全列（note 標「櫃・／第N次・／至 M/D」）。節標題用 `.dayhead` 樣式插在同一個 `#dynList` 內（保住輪播邏輯）；副標「今日／休市，順延」；`window.__TEST_TODAY='YYYY-MM-DD'` 可覆寫今天供測試。**除權息照抓、預設不顯示**，加回 `'dividend'` 即進上節（窗口內全列，列數多）；**清單不壓縮**——`#dynList` 解除 50vh 上限自然攤開、`#panelFixed{flex:none}` 防擠壓、整欄觸頂才輪播。
- 時區（2026-07-11 定案）：總經事件由資料層 `ts`（epoch）＋**系統時區**動態換算顯示（人在東京自動 +1h），頁腳動態標「本機時區（UTC±N）」；**台股面板「今天」也跟隨系統日期**（觀看者過了午夜即翻日、遇休市順延下一交易日——曾短暫釘台北，使用者實測後改回）；**只有資料層抓取窗口釘 Asia/Taipei**（python `datetime.now(TPE).date()`，防排程 00:00 JST 那輪濾掉台北當日事件——實際發生過）；時鐘與週範圍走系統時區。舊資料（無 ts）自動退回台北字串顯示。

## 環境限制（重要）

- ARM64 Windows。TradingView widget 與 iframe 嵌入在 WE 的 CEF 都會崩潰成死圖——**勿再嘗試**，總經維持自繪。
- Python：`C:\Users\ben03\AppData\Local\Programs\Python\Python311-arm64\`（排程用 pythonw.exe，無 py launcher）。該目錄雖在 User＋Machine 兩個 PATH，但 **schtasks `/TR` 不可用裸 `pythonw.exe`**——排程器不做 PATH 解析，會 0x80070002 靜默失敗（2026-07-10 實測），`/TR` 一律絕對路徑。

## 待辦（依序）

（目前無待辦。舊資料夾 finance-wallpaper 已於 2026-07-11 由使用者手動刪除。）

**版本慣例（2026-07-11 起）**：HTML 頂部 `const VERSION`，每批改動（≒每次 commit）手動遞增，顯示在時鐘卡右下角供確認 WE 套用成功；對照＝v4.0 f1e60e1、v4.1 44618d6、v4.2 4ae167b、v4.3 7efd686、v4.4 5f4ed43、v4.5 96a8faa。

（排程 2026-07-11 定案：**單一任務「TW財經桌布資料更新」**，定義檔＝版控內 `tw_update_task.xml`（每日 06:00 起每 6h＋登入後 2 分鐘＋錯過補跑 `StartWhenAvailable`＋電池可跑＋30 分鐘執行上限）。使用者已用管理員 PowerShell 套用並刪除過渡「-登入」任務，手動觸發實測 Last Result 0、資料檔翻新。教訓兩則：①任務由管理員建立後非管理員改不動（Access denied）；②**schtasks /XML 讀檔看實際位元組編碼**——宣告 UTF-16 但實存 UTF-8 會讓中文描述亂碼（實測），`tw_update_task.xml` 必須維持 UTF-16 LE 含 BOM（FF FE），Write 工具預設 UTF-8 寫完要用 `Set-Content -Encoding Unicode` 重存。0x800710E0＝啟動請求被任務條件拒絕，錯過不補跑與電池被擋兩種成因都實際遇過，XML 均已解除。教訓三（2026-07-15）：**開機補跑撞網路未就緒**——StartWhenAvailable 補跑在開機後約 1 分鐘執行、19 來源全敗但回傳碼仍 0（排程層失敗重試無從觸發；勿加 RunOnlyIfNetworkAvailable，同 0x800710E0 教訓），對策＝資料層 v4.4 韌性（等網路＋fallback）。）

## 可更新方向（backlog）

- 法說會改接 MOPS 即時來源（openapi 重大訊息快取偏舊；2026-07-11 目擊同日兩輪抓取 1 筆→0 筆，未來場次會從快取消失——若要穩定顯示，資料層可考慮把「已見過的未來場次」併入輸出）。
- 加 TPEx 上櫃除權息（tpex.org.tw/openapi）；行情條加櫃買指數（走 TPEx `/tpex_index`，Yahoo 已停更）。
- 興櫃處置：TPEx openapi `tpex_esb_disposal_information`（2026-07-11 已查證存在）。
- 注意股票類型：TWSE／TPEx openapi 均有注意股端點（2026-07-11 查證存在），可比照處置加一類。
- 台股「固定事件」遇假日順延標示（動態事件已接 holidays，固定事件尚未）。
- WE 滑桿追加：輪播速度、欄寬、事件視窗天數。
- 窄視窗（<1200px 雙欄 media query）尚未配置行情條版面（WE 實機不受影響）。
- 資料面板曾於重載後數分鐘短暫消失、數分鐘後自行恢復（2026-07-10 驗證目擊）：症狀已用 loadData 韌性修補根除（失敗沿用上次成功資料），**觸發源根因未明**——待開 WE CEF devtools 抓 console／頁面重載事件（懷疑與遮蔽暫停/恢復行為有關；2026-07-15 補：另一候選觸發源＝舊版非原子寫檔期間讀到半份 tw_events.js，v4.4 已改原子寫入，觀察是否再發）。
- 自製 preview.jpg 縮圖。

## 驗證

**WE 實機看動畫（時鐘跳動／清單輪播／行情條跑馬燈）前，先把該螢幕所有視窗取消最大化**——使用者效能設定 `playbackmaximized: pause` 會把頁面整個凍結（含時鐘），2026-07-11 曾因此誤判「滾動效果失效」查了一輪（chrome_debug.log 的「No task runner」刷屏＝WE 固有噪音、非錯誤；該 log 也不收 JS console 訊息）。真要看頁面內部狀態：WE 設定 → 一般 → devtools 命令列填 `--remote-debugging-port=8080` 重啟 WE，瀏覽器開 localhost:8080。
瀏覽器直接開 `finance-calendar.html`（有 fetch json fallback）；改完在 WE 重新點選桌布強制重載，或 CLI：`wallpaper64.exe -control openWallpaper -file <project.json>`（**注意：`applyProperties` 對本桌布無效**，2026-07-10 多變體實測；驗滑桿只能手動）。**切勿在 WE 用「開啟桌布→瀏覽 HTML 檔」**——會重造 project.json 蓋掉滑桿定義（2026-07-11 實際發生，`git restore project.json` 救回）。headless 截圖：`msedge --headless=new --screenshot=... --window-size=... --virtual-time-budget=6000 file:///...`。`python update_tw_events.py` 會列出各來源筆數與警告。
