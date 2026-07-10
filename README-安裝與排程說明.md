# 財經日曆桌布 — 說明

**本資料夾（Wallpaper Engine 專案）就是唯一來源**，所有檔案直接在這裡改，已用 git 管理版本。

## 檔案

| 檔案 | 用途 |
|---|---|
| `finance-calendar.html` | 桌布主體（可調參數在檔案最上方註解＋`CONFIG` 區塊） |
| `project.json` | WE 屬性定義：介面縮放／不透明度／模糊／強調色滑桿 |
| `bg.jpg` | 內建深色底圖（可換成任何圖片，或在 CONFIG 改純色／漸層） |
| `update_tw_events.py` | 抓總經日曆＋台股動態事件＋行情條五檔＋TWSE 休市日曆，輸出下面兩檔（零依賴、免裝套件） |
| `tw_events.json` / `tw_events.js` | 事件資料（排程自動更新，不進版控） |

## 版面與調整

整體錨定螢幕右上：中右＝時鐘＋總經日曆｜最右＝台股固定/動態事件｜底部橫貫＝行情條（12 檔：USD/TWD・美債10Y・WTI・加權・台積電・日經225・KOSPI・歐股50・道瓊・S&P 500・NASDAQ・SOFR 3M，紅漲綠跌、超寬自動水平輪播，`CONFIG.showQuotes` 可關；標的增減改 `update_tw_events.py` 頂部 `QUOTES`），左側全部留白給桌面圖示。台股動態事件只列「當日」（遇休市自動順延下一交易日）。
日常調整用 WE 屬性滑桿：已安裝桌布點一下 → 右側「介面縮放(%)／面板不透明度／毛玻璃模糊／強調色」，即時生效。縮放＝縮整體「佔用面積」（字體同縮、往右上收）；放大有防呆——高度不超出螢幕底、寬度超過視窗會自動夾到剛好塞下。進階（欄寬、顏色、重載時間）改 `finance-calendar.html` 的 `CONFIG`，改完在 WE 重新點選桌布。

桌布收不到滑鼠滾輪（在圖示層之下），清單超出高度會**自動慢速來回輪播**：速度 `autoScrollPxPerSec`（預設 24 px/秒，0＝關閉）、端點停留 `autoScrollPauseSec`（預設 4 秒）。

## 排程（每日 06:00 起、每 6 小時更新資料）

系統管理員 PowerShell：

```powershell
schtasks /Create /F /TN "TW財經桌布資料更新" /SC DAILY /ST 06:00 /RI 360 /DU 24:00 /TR '"C:\Users\ben03\AppData\Local\Programs\Python\Python311-arm64\pythonw.exe" "C:\Program Files (x86)\Steam\steamapps\common\wallpaper_engine\projects\myprojects\finance-calendar\update_tw_events.py"'
```

驗證：`schtasks /Run /TN "TW財經桌布資料更新"` 後看 `tw_events.json` 的 `updated` 時間。

## 資料來源

- 總經：Forex Factory 週曆 JSON（本週＋下週，含預測/前值；「下週」檔通常週末才發布，平日略過屬正常）。幣別與重要性在腳本頂部 `MACRO_CURRENCIES` / `MACRO_IMPACTS` 調整；常見指標已翻中文，冷門保留英文。原 TradingView widget 在 WE 的 CEF 會崩潰成死圖，故改自繪。
- 除權息：TWSE 除權除息預告表（即時 API），失敗退 OpenAPI 備援。
- 股東會：TWSE OpenAPI `t187ap41_L`（7 月非旺季，近兩週常為空屬正常）。
- 法說會：TWSE OpenAPI 重大訊息篩「第 12 款」。
- FinMind 需逐檔查詢無法一次撈全市場，故以 TWSE 為主。
- 行情條：Yahoo Finance chart API（免金鑰非官方端點；標的在腳本頂部 `QUOTES` 增減）。休市日曆：TWSE OpenAPI（僅當年度，跨年由週末規則兜底）。

## 自動更新機制

HTML 每 6 小時（`reloadHours`）重讀資料並重算固定事件；每天 04:00（`dailyReloadAt`）整頁重載讓「本週」視窗往前滾。固定事件（台指結算/季結算/財報/月營收截止）每次載入本地重算。資料超過 3 天未更新，面板會出現 ⚠ 提醒。

## 版本管理

```bash
git log --oneline          # 看歷史
git diff                   # 看未提交的修改
git add -A && git commit -m "說明"   # 提交
git checkout -- <檔案>     # 還原單檔
```

`tw_events.json/js`（資料）與 `preview.jpg`（WE 縮圖）已在 `.gitignore` 排除。

## 疑難排解

屬性滑桿沒出現＝重新點選桌布或重啟 WE。毛玻璃沒模糊＝舊版 CEF 不支援 `backdrop-filter`，把不透明度調高補償。總經/動態事件空白＝手動跑一次 `python update_tw_events.py` 看輸出訊息。改檔案後畫面沒變＝WE 重新點選該桌布強制重載。
