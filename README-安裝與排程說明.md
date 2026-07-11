# 財經日曆桌布 — 說明

**本資料夾（Wallpaper Engine 專案）就是唯一來源**，所有檔案直接在這裡改，已用 git 管理版本。

## 檔案

| 檔案 | 用途 |
|---|---|
| `finance-calendar.html` | 桌布主體（可調參數在檔案最上方註解＋`CONFIG` 區塊） |
| `project.json` | WE 屬性定義：介面縮放／不透明度／模糊／強調色滑桿 |
| `bg.jpg` | 內建深色底圖（可換成任何圖片，或在 CONFIG 改純色／漸層） |
| `update_tw_events.py` | 抓總經日曆＋台股動態事件＋行情條＋TWSE 休市日曆，輸出下面兩檔（零依賴、免裝套件） |
| `tw_events.json` / `tw_events.js` | 事件資料（排程自動更新，不進版控） |

## 版面與調整

整體錨定螢幕右上：中右＝時鐘＋總經日曆｜最右＝台股固定/動態事件｜底部橫貫＝行情條（12 檔：USD/TWD・美債10Y・WTI・加權・台積電・日經225・KOSPI・歐股50・道瓊・S&P 500・NASDAQ・SOFR 3M，紅漲綠跌、超寬時自動滾動——預設頭尾相接連續跑馬燈，`CONFIG.quotesLoop:false` 可改回到端點停留再反向的來回式；`CONFIG.showQuotes` 可關；標的增減改 `update_tw_events.py` 頂部 `QUOTES`），左側全部留白給桌面圖示。台股動態事件面板分上下兩節：**上節「法說會・股東會」列資料窗口（近兩週）內今天起的所有場次**（附日期，當天標「今天」）；**下節「處置股」維持當日制**（當日落在處置期間內全列、遇休市自動順延下一交易日；含上市＋上櫃、濾權證、標到期日；同檔二度處置只保留最新一筆並標「第N次」）。顯示類型在 `CONFIG.dynTypes` 設定，除權息資料照抓，把 `'dividend'` 加回陣列即在上節恢復顯示；清單自然攤開不壓縮，超出欄高才輪播。
時區：總經事件時間**跟隨電腦系統時區**顯示（頁腳標示目前偏移）；台股「今天」同樣跟隨系統日期（過了當地午夜即翻日、遇休市順延下一交易日）；資料抓取窗口固定錨在台北，出國改時區資料不會缺漏；時鐘顯示系統當地時間。
日常調整用 WE 屬性滑桿：已安裝桌布點一下 → 右側「介面縮放(%)／面板不透明度／毛玻璃模糊／強調色」，即時生效。縮放＝縮整體「佔用面積」（字體同縮、往右上收）；放大有防呆——高度不超出螢幕底、寬度超過視窗會自動夾到剛好塞下。進階（欄寬、顏色、重載時間）改 `finance-calendar.html` 的 `CONFIG`，改完在 WE 重新點選桌布。

桌布收不到滑鼠滾輪（在圖示層之下），清單超出高度會**自動慢速來回輪播**：速度 `autoScrollPxPerSec`（預設 24 px/秒，0＝關閉）、端點停留 `autoScrollPauseSec`（預設 4 秒）。

## 排程（單一任務 `TW財經桌布資料更新`，定義檔 `tw_update_task.xml`）

目標狀態＝一個任務、四重保障：每日 06:00 起每 6 小時、使用者登入後 2 分鐘、
錯過排程開機後補跑（`StartWhenAvailable`）、電池供電也執行（Surface 拔電源不斷更）。
完整定義在版控內的 `tw_update_task.xml`，建立／修復都用它（**系統管理員 PowerShell**）：

```powershell
schtasks /Create /F /TN "TW財經桌布資料更新" /XML "C:\Program Files (x86)\Steam\steamapps\common\wallpaper_engine\projects\myprojects\finance-calendar\tw_update_task.xml"
schtasks /Delete /F /TN "TW財經桌布資料更新-登入"   # 過渡期的獨立登入任務（併入主任務後移除，不存在會報錯屬正常）
```

驗證：`schtasks /Run /TN "TW財經桌布資料更新"` 後看 `tw_events.json` 的 `updated` 時間。

踩雷紀錄：

- 動作的執行檔**必須寫絕對路徑**——排程器不做 PATH 解析，裸 `pythonw.exe` 會以
  0x80070002（找不到檔案）靜默失敗（2026-07-10 實測）。
- 管理員權限建立的任務，非管理員 `schtasks /Create /F` 覆蓋會 Access denied
  （2026-07-11 實測）；改任務設定一律走管理員 PowerShell。
- 「上次執行結果 0x800710E0」＝啟動請求被任務條件拒絕，兩種情境都遇過（2026-07-11）：
  排程時刻電腦沒開機、開機後因沒開 `StartWhenAvailable` 拒絕補跑；或電池供電時
  被 `DisallowStartIfOnBatteries` 擋下。上面的 XML 兩者都已解除。

## 資料來源

- 總經：Forex Factory 週曆 JSON（本週＋下週，含預測/前值；「下週」檔通常週末才發布，平日略過屬正常）。幣別與重要性在腳本頂部 `MACRO_CURRENCIES` / `MACRO_IMPACTS` 調整；常見指標已翻中文，冷門保留英文。原 TradingView widget 在 WE 的 CEF 會崩潰成死圖，故改自繪。
- 除權息：TWSE 除權除息預告表（即時 API），失敗退 OpenAPI 備援。
- 股東會：TWSE OpenAPI `t187ap41_L`（7 月非旺季，近兩週常為空屬正常）。
- 法說會：TWSE OpenAPI 重大訊息篩「第 12 款」。
- 處置股：上市 TWSE OpenAPI `announcement/punish`＋上櫃 TPEx OpenAPI `tpex_disposal_information`（皆為固定近期窗口；只收股票/ETF、濾權證）。
- FinMind 需逐檔查詢無法一次撈全市場，故以 TWSE 為主。
- 行情條：Yahoo Finance chart API（免金鑰非官方端點；標的在腳本頂部 `QUOTES` 增減）。休市日曆：TWSE OpenAPI（僅當年度，跨年由週末規則兜底）。

## 自動更新機制

HTML 每 6 小時（`reloadHours`）重讀資料並重算固定事件；另外每 5 分鐘（`freshCheckMin`）輕量重讀資料檔、`updated` 有變才重繪——開機時頁面先載入舊檔，登入排程約 2 分鐘後寫入新檔，靠這個在幾分鐘內把新資料換上桌（也不會無謂打斷清單輪播）；每天 04:00（`dailyReloadAt`）整頁重載讓「本週」視窗往前滾。固定事件（台指結算/季結算/財報/月營收截止）每次載入本地重算。資料超過 3 天未更新，面板會出現 ⚠ 提醒。

## 版本管理

```bash
git log --oneline          # 看歷史
git diff                   # 看未提交的修改
git add -A && git commit -m "說明"   # 提交
git checkout -- <檔案>     # 還原單檔
```

`tw_events.json/js`（資料）與 `preview.jpg`（WE 縮圖）已在 `.gitignore` 排除。

## 疑難排解

資料更新了但桌布沒變＝正常，HTML 每 6 小時才重讀資料；要立即生效請在 WE 清單**重新點選本桌布**，或跑「驗證」段的 CLI 指令。**切勿用「開啟桌布 → 瀏覽選 HTML 檔」**——WE 會把資料夾當新專案、重造 project.json 蓋掉四個滑桿定義（2026-07-11 踩過；被蓋掉時 `git restore project.json` 可救回）。
屬性滑桿沒出現＝重新點選桌布或重啟 WE。毛玻璃沒模糊＝舊版 CEF 不支援 `backdrop-filter`，把不透明度調高補償。總經/動態事件空白＝手動跑一次 `python update_tw_events.py` 看輸出訊息。改檔案後畫面沒變＝WE 重新點選該桌布強制重載；**時鐘卡右下角有版本小字**（HTML 頂部 `VERSION`，每批改動遞增），重載後看版號有沒有跳＝新版是否套用成功。
