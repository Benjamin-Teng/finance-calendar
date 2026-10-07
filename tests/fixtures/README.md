# 測試樣本（真實錄製回應）

`tests/test_wallpaper_data.py` 與 `tests/compare_lively_dom.mjs` 使用的樣本，全部是真實回應、未經改寫
（`tw_events_sample.json` 除外，見下表）。錄製日 2026-10-02（週五），台北時間 10:46–10:55，盤中。
錄製用的 User-Agent 同 `update_tw_events.py` 的 `UA`。

| 檔案 | 來源 URL | 錄製時間（台北） | 用途 |
| --- | --- | --- | --- |
| `yahoo_twii_5m_5d.json` | `https://query1.finance.yahoo.com/v8/finance/chart/%5ETWII?interval=5m&range=5d` | 10:48 | 盤中走勢；含 9/24、9/29、9/30、10/1 四個完整日，與 10/2 的未收盤部分序列（09:00–10:27，末筆是 Yahoo 另附的即時報價） |
| `yahoo_twii_1d_3mo.json` | `https://query1.finance.yahoo.com/v8/finance/chart/%5ETWII?interval=1d&range=3mo` | 10:48 | 日 K，共 65 筆：63 個已收盤日＋10/2 盤中那根（close 為即時價）＋7/10 一筆 OHLC 為 null |
| `margn_ms_20260930.json` | `https://www.twse.com.tw/rwd/zh/marginTrading/MI_MARGN?date=20260930&selectType=MS&response=json` | 10:46 | 券資比 2.53% 的彙總（規格指定 9/30） |
| `margn_all_20260930.json` | 同上，`selectType=ALL` | 10:46 | 9/30 個股融資融券明細（1297 檔） |
| `margn_ms_20261001.json`、`margn_all_20261001.json` | 同上，`date=20261001` | 10:48 | 10/1 彙總與明細，與下一列的收盤價同日 |
| `stock_day_all_20261001.json` | `https://openapi.twse.com.tw/v1/exchangeReport/STOCK_DAY_ALL` | 10:48 | 收盤價，`Date` 欄為 `1151001`（10/1，最新一個交易日） |
| `holiday_schedule_20261002.json` | `https://openapi.twse.com.tw/v1/holidaySchedule/holidaySchedule`（以 `update_tw_events.py` 的 `UA` 抓取） | 14:32（台北，補錄） | `HolidaysTest`：2026 全年 27 列，含「開始交易日／最後交易日」說明列（1/2、2/11、2/23）與「市場無交易，僅辦理結算交割作業」（2/12、2/13） |
| `holiday_schedule_rwd_2021_20261002.json` … `holiday_schedule_rwd_2025_20261002.json`（5 檔） | `https://www.twse.com.tw/rwd/zh/holidaySchedule/holidaySchedule?date=<YYYY>0101&response=json`（TWSE 網站同源端點，`<YYYY>`＝2021–2025；欄位 `日期／名稱／說明` 對應 openapi 的 `Date／Name／Description`，日期為西元 ISO） | 14:36（台北，補錄） | `HolidaysTest`：歷年變體。2021／2022 的結算交割日 Name 也叫「農曆春節前最後交易日」、只在說明寫「市場無交易」；2023-01-17 是交易日，說明第二句提到 1/18、1/19 無交易。測試層把 rwd 列轉成 openapi 形狀，正式程式仍只抓 openapi |
| `tw_events_sample.json` | 以 `uv run --no-project python update_tw_events.py <暫存資料夾>`（`LOCALAPPDATA` 指向不存在的路徑，避免鏡像到 Lively）實跑產出的 `tw_events.json` | 10:55（`holidays` 鍵 14:59 重錄） | `compare_lively_dom.mjs` 的輸入（含三個新鍵；腳本會產生「不含新鍵」那份比對）。`holidays` 鍵在資料層修正（commit `ca44edb`）後以同一指令重跑、只替換這個鍵：24 筆，不再含交易日 1/2、2/11、2/23（task 3.4 修正輪 1）；其餘鍵維持 10:55 錄製值 |

## 為什麼 `STOCK_DAY_ALL` 沒有 9/30

`STOCK_DAY_ALL` 只回「最新一個交易日」，不接受日期參數：實測 openapi 端點本來就沒有參數；`rwd/zh/afterTrading/STOCK_DAY_ALL?date=20260930`
也被忽略，回的仍是 `1151001`。錄製時最新交易日是 10/1，所以收盤價樣本是 10/1。

因此「融資維持率」的完整計算（同日價格＋同日融資）改用 **10/1** 一組同日樣本驗證（`margn_*_20261001.json`＋`stock_day_all_20261001.json`）。
9/30 的樣本用在：

- 券資比 2.53%：融資 9,286,318 張、融券 235,296 張，235,296 ÷ 9,286,318 × 100 ＝ 2.5338%。
- 「收盤價日期（10/1）≠ 融資日期（9/30）時不混用、沿用舊值」：這是真實存在的錯位組合，不是人造的。

另有一個可指定日期的官方收盤價來源：`rwd/zh/afterTrading/MI_INDEX?date=20260930&type=ALLBUT0999`
（實測可回 9/30 全市場收盤，加權收盤 47,940.13，與 Yahoo 日 K 一致）。實作沒有採用它：規格與 design D10 明訂搭配
`STOCK_DAY_ALL`，且資料層以「價格日期須等於融資日期、否則沿用」處理錯位，更簡單。若日後發現晚間公布後仍長期錯位，
可改接這個端點。

## 查證結果

### Yahoo `interval`／`range`（task 2.1）

Yahoo v8 chart 沒有官方文件，以下全部由實測回應與錯誤訊息查證（2026-10-02，`^TWII`）：

- 盤中：`interval=5m&range=5d`。回應 `meta.dataGranularity` ＝ `5m`、`meta.range` ＝ `5d`、
  `meta.validRanges` ＝ `1d、5d、1mo、3mo、6mo、1y、2y、5y、10y、ytd、max`。
  - `range=5d` ＝最近 **5 個交易日**（不是日曆日）：實測得 9/24、9/29、9/30、10/1、10/2，中間跨過 9/25、9/28 兩個休市日。
    連假後第一個盤中也撈得到上一個完整交易日。
  - 每個完整日 54 根 K，09:00–13:25（整點起算的 5 分鐘 K，13:25 那根涵蓋到 13:30），相鄰間隔全為 300 秒，沒有 null。
    最後一根的收盤價不含 13:30 收盤試撮，與官方收盤價不同（10/1：最後一根 48,281.21，官方收盤 48,353.49）。
  - 5m 資料只提供最近 60 天：`interval=5m&range=3mo` 回 `Unprocessable Entity：5m data not available … The requested range must be within the last 60 days`。
  - 1m 只提供最近 8 天：`interval=1m&range=1mo` 回 `Only 8 days worth of 1m granularity data are allowed`。
    規格只要求相鄰間隔 ≤ 5 分鐘，故不用 1m。
  - `range=1d` 只含當日（實測只有 10/2），中午那輪沒有前一個完整交易日可用，所以不能用。
- 日 K：`interval=1d&range=3mo`，`dataGranularity` ＝ `1d`，實測 65 筆（63 個已收盤日＋當日盤中＋1 筆 null），遠超 40 根的下限。
  9/30 收盤 47,940.13、10/1 收盤 48,353.49，與 TWSE `MI_INDEX` 的 9/30 加權收盤 47,940.13 一致。
- 時間戳是 UTC epoch；`meta.gmtoffset` ＝ 28800（+08:00）。資料層一律以固定 +08:00 的 `TPE` 換算，不用系統時區。

### `MI_MARGN`（融資融券）

- 端點：`https://www.twse.com.tw/rwd/zh/marginTrading/MI_MARGN`。`selectType=MS` 只回彙總表；`ALL` 回彙總表＋個股明細表
  （表標題「融資融券彙總 (全部)」，欄位 `代號、名稱、融資[買進、賣出、現金償還、前日餘額、今日餘額、次一營業日限額]、融券[同左]、資券互抵、註記`）。
- 日期：回應頂層 `date` ＝ `YYYYMMDD` 西元（如 `20260930`）。不帶 `date` 回最新已公布的那天；帶 `date`
  指定日期可取歷史（9/30、10/1 實測成功）；非交易日或尚未公布回 `{"stat":"很抱歉，沒有符合條件的資料"}`
  （實測 `date=20261002`〔10/2 上午、尚未公布〕與 `date=20260928`〔補假日〕皆如此）。
- 單位：「融資(交易單位)」「融券(交易單位)」＝**張**（1 交易單位 ＝ 1000 股）；「融資金額(仟元)」＝**仟元**。
  交叉驗證：9/30 個股融資今日餘額加總 9,286,318 ＝ 彙總 9,286,318；融券 235,296 ＝ 235,296，兩天皆完全相等。
  以張×1000×收盤價算出的市值 ÷ 融資金額（仟元×1000）≈ 195%，量級合理（若單位是股會差 1000 倍）。
- 欄位取用：規格的 2.53% 用的是「**今日餘額**」。表尾註記說「請以『前日餘額』為準，『今日餘額』為輔助參考」，
  那是指當日數字之後可能被次日調帳修正；但「前日餘額」要等下一個交易日才有，與當日收盤價對不上，故採今日餘額。
- 晚間公布：註記寫明「當日晚間即時公布」，公布前 `MS` 回的是前一個交易日。

### `STOCK_DAY_ALL`

- 端點：`https://openapi.twse.com.tw/v1/exchangeReport/STOCK_DAY_ALL`（swagger 摘要「上市個股日成交資訊」）。
- 欄位：`Date`（民國 7 碼，`1151001`）、`Code`、`Name`、`TradeVolume`、`TradeValue`、`OpeningPrice`、`HighestPrice`、
  `LowestPrice`、`ClosingPrice`（字串）、`Change`、`Transaction`。單位：收盤價＝元。
- 無成交（停牌、處置暫停等）的個股 `ClosingPrice` 為空字串：10/1 樣本共 1380 筆，其中融資有餘額、卻查無收盤價的有 7 檔
  （`00666R`、`00707R`、`1589`、`2323`、`2601`、`6655`、`910322`），依規格略過；ETF（如 `0050`）有收盤價、計入。
