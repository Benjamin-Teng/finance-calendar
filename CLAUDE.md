# finance-calendar — Wallpaper Engine 財經日曆桌布

金十桌面風格的 WE 網頁桌布。本資料夾（WE 專案）＝唯一來源，回覆一律繁體中文。

## 現況（2026-07-10）

- `finance-calendar.html` 單檔自繪，深色毛玻璃。版面整體錨定右上（`.layout` 為 `position:fixed; top:0; right:0` 內容尺寸容器＋`transform:scale(var(--z))`、origin 右上）：中右＝時鐘＋總經日曆、最右＝台股固定＋動態事件，左側留白給桌面圖示。所有參數在頂部 CONFIG。
- 資料層 `update_tw_events.py`（純 stdlib、零依賴）→ 產出 `tw_events.js/.json`（.gitignore 已排除）：
  - 總經＝ForexFactory 週曆 JSON（thisweek＋nextweek；nextweek 週末才發布、平日自動略過不報錯；USD/EUR/JPY、中高重要性、轉台北時區、內建約 80 條指標中譯字典）
  - 除權息＝TWSE TWT48U 即時 API（備援 openapi TWT48U_ALL）；股東會＝openapi t187ap41_L；法說會＝openapi t187ap04_L 篩「第12款」
  - 行情＝Yahoo v8 chart（免金鑰；UA 換掉 Python 預設值即可、免 crumb；欄位 `meta.regularMarketPrice`／`chartPreviousClose`——v8 沒有 `previousClose` 這個鍵）。標的在檔頂 `QUOTES`，預設 12 檔：USD/TWD、US 10Y、WTI、加權、台積電、日經225、KOSPI、歐股50、道瓊、S&P 500、NASDAQ（以上 Yahoo）＋SOFR 3M（NY Fed `sofrai/last/2` 90 天複合平均，`fetch_sofr()` 另抓、append 在最後；日變動極小，前端捨入為 0.00 時顯示灰色持平）。櫃買指數 Yahoo 已停更，要加走 TPEx openapi `/tpex_index`。stooq 備援已死（全站 JS PoW 反爬，2026-07-10 實測）
  - 休市日曆＝TWSE openapi holidaySchedule（民國年 7 碼、**只回當年度**，跨年由前端週末規則兜底）→ 輸出鍵 `holidays`
- WE 屬性滑桿（`project.json`，鍵名必須全小寫）：uiscale／panelopacity／blurpx／accentcolor → HTML `wallpaperPropertyListener` 接收。
- 縮放：`setScale()` 設 CSS 變數 `--z`，整體佔用面積等比縮放（縮小往右上收）。防呆：欄高上限 `calc(min(92vh, 92vh/--z) - 52px)`（52＝容器上下 padding）防底部裁切；寬度夾限 z ≤ (innerWidth−8)/約1052px 防窄視窗左緣裁切（掛 resize 重算）。
- 清單超高自動來回輪播 `setupAutoScroll()`（桌布在圖示層下收不到滾輪；2026-07-10 修正端點判斷未含行進方向的死循環——此功能此前從未真正動過）；HTML 每 6h 重讀資料、每日 04:00 reload；台股固定事件（第三週三台指結算、3/6/9/12 第三週五季結算、財報 3/31・5/15・8/14・11/14、每月 10 日營收截止）純本地計算。
- 底部行情條（2026-07-10 新增）：`.layout` 第三格 `grid-column:1/-1`，`quotes` 鍵驅動、紅漲綠跌（`--up`/`--dn`）、yield 類漲跌顯示絕對值、超寬時 `setupAutoScrollX()` 水平來回輪播、無資料自動隱藏；`CONFIG.showQuotes` 開關；顯示時以 root 變數 `--qh` 從兩欄高度預算讓位。
- 台股動態事件當日制（2026-07-10 新增）：只列目標日（今天；非交易日順延下一交易日，`holidays`＋週末判定、掃描上限 30 天）；副標顯示「今日／休市，順延」；`window.__TEST_TODAY='YYYY-MM-DD'` 可覆寫今天供測試（僅影響此邏輯）。

## 環境限制（重要）

- ARM64 Windows。TradingView widget 與 iframe 嵌入在 WE 的 CEF 都會崩潰成死圖——**勿再嘗試**，總經維持自繪。
- Python：`C:\Users\ben03\AppData\Local\Programs\Python\Python311-arm64\`（排程用 pythonw.exe，無 py launcher）。

## 待辦（依序）

1. **建工作排程**（尚未建立）：README「排程」段有完整 schtasks 指令（每日 06:00 起每 6h，pythonw 無視窗）。
2. 確認一切正常後刪除舊資料夾 `C:\Users\ben03\Claude\Projects\財經知識學習\finance-wallpaper`。

## 可更新方向（backlog）

- 法說會改接 MOPS 即時來源（openapi 重大訊息快取偏舊）。
- 加 TPEx 上櫃除權息（tpex.org.tw/openapi）；行情條加櫃買指數（走 TPEx `/tpex_index`，Yahoo 已停更）。
- 台股「固定事件」遇假日順延標示（動態事件已接 holidays，固定事件尚未）。
- WE 滑桿追加：輪播速度、欄寬、事件視窗天數。
- 窄視窗（<1200px 雙欄 media query）尚未配置行情條版面（WE 實機不受影響）。
- 資料面板曾於重載後數分鐘短暫消失、數分鐘後自行恢復（2026-07-10 驗證目擊）：症狀已用 loadData 韌性修補根除（失敗沿用上次成功資料），**觸發源根因未明**——待開 WE CEF devtools 抓 console／頁面重載事件（懷疑與遮蔽暫停/恢復行為有關）。
- 自製 preview.jpg 縮圖。

## 驗證

瀏覽器直接開 `finance-calendar.html`（有 fetch json fallback）；改完在 WE 重新點選桌布強制重載，或 CLI：`wallpaper64.exe -control openWallpaper -file <project.json>`（**注意：`applyProperties` 對本桌布無效**，2026-07-10 多變體實測；驗滑桿只能手動）。headless 截圖：`msedge --headless=new --screenshot=... --window-size=... --virtual-time-budget=6000 file:///...`。`python update_tw_events.py` 會列出各來源筆數與警告。
