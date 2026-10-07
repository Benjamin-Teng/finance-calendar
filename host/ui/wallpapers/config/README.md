# 主題設定檔內建預設值（wallpaper-config）

動態桌布各主題共用的設定：六市場時段表、各市場年度休市表、訂單旺季、財報密集週、宜忌字句與詞庫、門檻。
**鍵名與結構就是契約**：星盤（task 3.3）與撕日曆（task 3.4）直接讀它。

## 檔案

| 檔案 | 用途 |
|---|---|
| `wallpaper-config.default.json` | 內建預設值。task 4.1 把它編進宿主執行檔；使用者的 `wallpaper-config.json` 讀進來後與它做**頂層鍵**合併（design.md D7，見下方「合併行為」）。獨立模式下繪圖頁經 `{data, config}` 信封把它當 `config` |
| `wallpaper-config.schema.json` | JSON schema（draft 2020-12），約束「完整檔」：必填欄位、型別、市場 id、日期 `YYYY-MM-DD`、時間 `HH:MM`、門檻為正數 |
| `../lib/config-holidays.mjs` | 休市表語意與讀取端檢查：`validateThemeConfig`、`holidayOn`／`holidayOnAt`、`holidayExpiryReminder`、`unverifiedHolidayYears` |
| `../../../tests/wallpapers/config-default.test.mjs` | schema 表達不了的檢查（日期是否真實存在、是否週末、出處是否解得開、字句長度與用詞、時段表與樣稿逐欄相同） |
| `../../../tests/wallpapers/config-holidays.test.mjs` | 上述語意與檢查函式的單元測試 |
| `../../../tests/wallpapers/config-invalid/`、`config-valid/` | 故意錯誤的樣本（schema 必須擋）與合法編輯樣本（schema 必須放行）；`config-invalid/generate.mjs` 由預設檔重新產生 |
| `../../../tests/wallpapers/check-config-schema.sh` | 一次性執行 `uvx check-jsonschema`：預設檔與合法樣本要過、每份錯誤樣本都要被擋 |

驗證指令（在 repo 根目錄）：

```text
bash host/tests/wallpapers/check-config-schema.sh
node --test "host/tests/wallpapers/*.test.mjs"
```

## 鍵結構

```text
version                       結構版本（整數）
markets[]                     六市場時段表，照搬樣稿 bg-sessions.html 的 MARKETS（順序 TYO SEL TPE HKG LON NYC）
  id name tz color src
  segments[]                  kind(regular|auction|pre|post|night) start end days label [skip] [effective]
holidays                      市場 id → 西元年 → { through, days[] }
  <id>.<year>.through         官方日曆已知到此日（含）
  <id>.<year>.verified        選填；false＝待複核
  <id>.<year>.days[]          { date, name, src? } 或半日市 { date, name, half:true, close, src? }
holidaySources                出處登記簿：id → { title, url, checked, note?, related?[] }
lunarNewYear                  選填。撕日曆干支年的換年日（農曆正月初一）
  dates.<year>                { date: "YYYY-MM-DD", src }：該西元年的初一（必須在同年 1/21–2/20）
  sources                     出處登記簿（格式同 holidaySources）
orderSeasons[]                { label, months[] }：7–9 月「電子旺季」、10 月「年底備貨」
earningsWeek                  { deadlines: ["03-31","05-15","08-14","11-14"], leadDays: 7 }
thresholds                    { shortRatio: 2, maintenanceRatio: 150 }（百分比，必須為正數）
phrases
  closedDay.yi                休市日「宜」（單行）
  closedDay.ji[]              休市日「忌」（每元素一行）
  marginWarning.ji            融資警示「忌」
  events.tsmcEarnings         「台積電財報」
  events.earningsWeek         「財報密集週」
  yiPool[] jiPool[]           詞庫，依日期取一句、同一天固定同一句
```

### 農曆正月初一表（`lunarNewYear`，task 3.4）

撕日曆的干支年以農曆正月初一換年。列在 `lunarNewYear.dates` 的年度以表中日期為準；沒列的年度改用
`Intl.DateTimeFormat('zh-TW-u-ca-chinese')`（ICU）。干支名稱一律由 Intl 提供，表中只放換年日期。

需要這張表，是因為 ICU 的天文近似在「朔」接近午夜的年份會算錯一天。例如 2027 年的朔約在北京時間 23:56，
ICU 把初一算成 2/7，官方日期是 **2/6**。

內建 2026–2028 年三筆，都對照中央氣象署「日曆資料表」（國農曆對照 PDF，`https://www.cwa.gov.tw/Data/astronomy/<年>cal.pdf`），
查證日期 2026-10-02：

| 年 | 初一 | 出處 |
| --- | --- | --- |
| 2026 | 2026-02-17（二） | 中央氣象署 115 年日曆資料表 |
| 2027 | 2027-02-06（六） | 中央氣象署 116 年日曆資料表；人事行政總處 116 年辦公日曆表附表 1「農曆初一（2/6）」。ICU 算成 2/7 |
| 2028 | 2028-01-26（三） | 中央氣象署 117 年日曆資料表 |

新的年度公布後，照同一格式補上 `dates.<年>` 與 `sources`。

`validateThemeConfig` 會對下列情況回警告，該年改用 Intl：

- 日期不存在。
- 日期不在該年 1/21–2/20。
- `src` 不在 `sources` 內。

### 撕日曆字句長度

撕日曆的「宜」「忌」每一項都畫成單行。字句太長時，該行字級會縮小到放得下為止，不截斷、不換行。

字句超過 **20 字**時，`validateThemeConfig` 會回警告。字數以字元計，全形空格也算一個字。檢查範圍：

- `phrases` 的所有字句
- `orderSeasons[].label`

即使超過 20 字，頁面仍照畫。但大約超過 24 字時，字級會低於可讀下限（可用寬度 ÷ 24），頁面會另外記警告。

### 休市表語義（契約，3.3／3.4／4.1／4.8 依此實作）

1. **`date` 是交易所當地的日曆日**，以該市場在 `markets[].tz` 的時區判定，不是台北或本機日期。例：台北 11-27 07:00
   時紐約仍是 11-26（感恩節）；用本機日期查會錯位一天。查表用 `holidayOnAt(config, 市場, nowMs)`（內部依 tz 換算）。
2. **`days` 內明列的日期一律算休市日或半日市，不受 `through` 限制。** `through` 只說明官方日曆已知到哪天。
3. **晚於 `through`（或該年度鍵不存在）且未列出的日期是「未知」：畫成一般交易日、不變暗**，只用於到期提醒
   （`holidayOn` 回 `status: 'unknown'`，畫面上等同開市）。TPE 2027 目前就是「未知」，撕日曆的「台股休市日」判斷照常依週末規則。
4. **年底提醒（4.8）**：本機日期距 12/31 **不足** 30 天（12/02 起；12/01 剛好 30 天不提醒），且任一市場的最晚 `through` 早於**明年** 12/31
   （沒有明年的鍵也算）→ 提示更新。`holidayExpiryReminder(config, 今天)` 回傳需要更新的市場 id。部分公布的年度（HKG 2027 只到 10-08）
   同樣會觸發。
5. **使用者在 `through` 之後新增休市日時，請同時把該年度的 `through` 延伸到已涵蓋的日期。** 沒延伸時日期仍算休市
   （不會被丟掉；`mergeConfig` 對 `days` 陣列整份取代、`through` 缺鍵由預設補上），但 `validateThemeConfig` 會回警告。
6. **半日市**（`half: true`）當天照常開市、提早收。`close` ＝交易所當地時間（NYSE 13:00、LSE 12:30、HKEX 12:00）。
   精確規則：**當天非 `night` 的時段一律截到 `close`；起點 ≥ `close` 的非 `night` 時段不畫；`night`（期貨）時段照畫。**
   效果：`regular` 在 `close` 結束，收盤 `auction` 與 `post` 自然不畫，開盤 `auction`（例：LSE 07:50）與盤前保留；
   收盤時刻（「接下來」的收盤）就是 `close`。已知簡化：期貨在半日市的實際收盤時間（例：HKEX 指數期貨 12:30）不建模。
   **跨午夜的時段（夜盤、紐約 Overnight 等）以時段「起點」的交易所當地日期查休市表**：起點那天休市＝整段不畫；
   起點那天不休市＝整段照畫，即使跨進休市日（例：感恩節前一晚 18:00 開始的 E-mini 照畫到感恩節早上 09:30）。
   **已知簡化**：個別交易所在休市日前一晚是否停開夜盤（例：台指期在連假前一晚的安排）不建模。
   星盤的狀態另以「交易所當地的今天」查表：今天休市＝整條環變暗、標示「休市（假日名）」，優先於仍在進行的前一晚時段。
7. **`verified: false`**（年度層，選填）＝該年度資料待與官方複核，未寫視為已核對；`through` 仍照官方資料填。目前只有 SEL 2027。
   4.8 可用 `unverifiedHolidayYears(config)` 在設定視窗提示。
8. 只列週一至週五，週末不列；颱風假屬臨時休市，不列。補假、順延的日子照官方列示，名稱以「（補假）」標示；名稱用台灣慣用譯名。
9. `src` 是 `holidaySources` 的 id，逐筆指到官方出處。**內建預設值每一筆都有 `src`**（單元測試把關）；使用者自行新增的休市日可省略。
10. **`earningsWeek.leadDays` 含截止日當天**：`leadDays=7` 表示窗口為 `deadline-6 … deadline`，共 7 個日曆日
    （例：截止日 08-14 → 08-08 至 08-14）。

### 讀取端檢查（`lib/config-holidays.mjs`）

schema 表達不了、使用者檔在執行期又不經 schema 的檢查，由 `validateThemeConfig(config)` 負責：回傳**警告字串陣列、不拋錯**，
**由 task 4.1 接線**：繪圖頁的 `env.withDefaults`（`lib/wallpaper.mjs`）一律呼叫 `mergeThemeConfig`＝`mergeConfig` 後再跑本函式，警告經 `env.warn` 併入頁面 warnings、隨 `meta.warnings` 交宿主記錄（宿主 Rust 只做頂層鍵合併、不重寫這些檢查，見下方「合併行為」）。內建預設零警告。涵蓋：市場表元素不是物件、缺 `id`、`tz` 不是 Intl 認得的 IANA 時區（task 4.1）、不存在的日期（如 2026-02-30）、年度鍵與日期年份不符、
日期晚於 `through`、同年度日期重複、`holidays` 的市場 id 不在 `markets` 內、`half` 與 `close` 配對、`src` 不在 `holidaySources`、
`through` 不合法、財報截止日不存在、門檻非正數。同一模組另有 `holidayOn`／`holidayOnAt`（查表）、`holidayExpiryReminder`（年底提醒）、
`unverifiedHolidayYears`（待複核年度）。

查表函式一律不拋錯（task 4.1）：`markets`／`days` 裡不是物件的元素略過，`tz` 不認得或時刻不是有限數字時回 `{ status: 'unknown' }`
（畫面上等同開市）；訊息加進選填的最後一個參數 `warnings`（陣列或 Set）。星盤另在 `computeAstrolabe` 略過壞的市場元素並記警告。

### schema 與未知鍵

- **頂層允許未知鍵**（`mergeConfig` 原樣保留，使用者可放自己的備註）；**巢狀物件一律不允許未知鍵**，用來抓拼錯的鍵名
  （例：`shortRatioo`）。`mergeConfig` 本身在每一層都保留未知鍵，所以 schema 比執行期嚴格，執行期防線是上面的 `validateThemeConfig`。
- `holidays` 的市場 id 與 `markets[].id` 限定為 `TYO`、`SEL`、`TPE`、`HKG`、`LON`、`NYC`；日期有 `pattern` 與 `format: date`
  （後者讓 check-jsonschema 順帶擋掉 2026-02-30）。
- `holidays[].days[].src` 在 schema 中選填。

### 合併行為（宿主頂層合併＋頁面 `mergeConfig`）

合併分兩層（task 4.1）：

1. **宿主（Rust，`host/src/wallpaper_config.rs`）只做頂層鍵**：使用者檔的頂層鍵型別與內建預設相同就整個採用使用者的值，
   型別不同改用預設並在記錄檔留警告，缺的頂層鍵補預設並記警告（每次載入一行、點名全部缺鍵），多出來的頂層鍵原樣保留。設定檔損壞（不是合法 JSON 或頂層不是物件）
   時整份改用內建預設、記警告，**使用者的檔案不覆寫也不搬走**；缺檔時宿主啟動會寫出一份完整的內建預設供編輯。
2. **頁面 `mergeThemeConfig`（`mergeConfig`＋`validateThemeConfig`）負責巢狀內容**，是巢狀補齊與驗證的單一事實來源，
   由 `env.withDefaults` 呼叫，巢狀錯誤的警告經 `meta.warnings` 進宿主記錄。下面的規則都是 `mergeConfig` 這一層。

因此宿主送給頁面的 `holidays` 可能只有使用者寫的那幾個年度；其他年度由頁面與自己的內建預設合併補回。
凡是在頁面以外讀這份設定的地方（例如 4.8 設定視窗的年底提醒），同樣要先經 `mergeThemeConfig`（或至少 `mergeConfig`）。

頁面 `mergeConfig`：物件逐鍵遞迴合併、**陣列整個取代**、型別不符退回預設並警告。因此：

- `holidays` 是「市場 → 年度」的巢狀物件，使用者可只補新年度（例：`{"holidays":{"TPE":{"2027":{"through":"2027-12-31","days":[...]}}}}`），
  不會蓋掉其他年度。補**既有**年度的 `days` 時，陣列整份取代，且要依第 5 點同時給 `through`。
- `markets[]`、`orderSeasons[]`、各詞庫是陣列，使用者一旦提供就整份取代（可以刪項目）。

## 休市表的官方出處（查證日期 2026-10-02）

| 市場 | 官方來源 | 2026 | 2027 |
|---|---|---|---|
| TPE（TWSE＋TAIFEX） | 證交所「市場開休市日期」：openapi `https://openapi.twse.com.tw/v1/holidaySchedule/holidaySchedule`；網頁 `https://www.twse.com.tw/zh/trading/holiday.html` | 全收（18 筆休市） | **未收**：查詢 `date=2027` 回「116 年」但 0 筆，尚未公布 |
| TYO（JPX） | 日本取引所グループ「営業時間・休業日一覧」`https://www.jpx.co.jp/corporate/about-jpx/calendar/index.html`（英文 `/english/corporate/about-jpx/calendar/index.html`） | 全收（19 筆） | 全收（17 筆），頁面已公布 |
| SEL（KRX） | 한국거래소 휴장일 `https://open.krx.co.kr/contents/MKD/01/0110/01100305/MKD01100305.jsp`，資料取自頁面 API `POST https://open.krx.co.kr/contents/OPN/99/OPN99000001.jspx`（`bld=MKD/01/0110/01100305/mkd01100305_01`、`search_bas_yy`） | 全收（17 筆） | 收（16 筆）。**待複核**（`verified: false`）：API 已回傳，但官網年度下拉選單只到 2026 |
| HKG（HKEX） | HKEX Calendar `https://www.hkex.com.hk/News/HKEX-Calendar?sc_lang=en`（資料內嵌於頁面）；半日市收盤時間 `https://www.hkex.com.hk/Services/Trading-hours-and-Severe-Weather-Arrangements/Trading-Hours/Securities-Market?sc_lang=en` | 全收（14 筆休市＋3 筆半日市） | **部分收**：頁面只涵蓋至 2027-10-08（`through` 即此日）；**11–12 月待查證** |
| LON（LSE） | LSE「Business days」`https://www.londonstockexchange.com/equities-trading/business-days`（JS 渲染）；2026 年 8 月前已從該頁消失的日期取自其連結的結算日曆 `https://docs.londonstockexchange.com/sites/default/files/documents/settlement-calendar-to-1-jan-2027-v2.xlsx`（CREST GBP 欄，文件更新於 2026-01-28） | 全收（8 筆休市＋2 筆半日市） | 全收（8＋2），頁面已公布 |
| NYC（NYSE） | NYSE「Holidays & Trading Hours」`https://www.nyse.com/markets/hours-calendars`（JS 渲染，列 2026–2028） | 全收（10＋2） | 全收（10＋1） |

### 待查證與已知限制

- **TAIFEX 行事曆**（`https://www.taifex.com.tw/file/taifex/CHINESE/4/2026Calendar.pdf`）是圖形月曆，無法機器比對；
  期交所休市日採證交所列表，兩者一致性待人工核對。
- **LSE 2026 年 8 月前的日期**由結算日曆推得（GBP 結算休日）。LSE 官方頁敘述「NON-trading day」同時是「GBX/GBP
  非結算日」，兩者一致；若 LSE 日後恢復在頁面保留已過去的日期，應改引頁面。
- 半日市以外的特殊交易日（延後開盤、縮短時段等）未建模，不屬休市表範圍。

## 每年更新休市表

spec 要求「休市表缺少下一年度資料且距年底不足 30 天時，設定視窗提示更新」。更新步驟（各交易所公布時間不一，建議 11 月起每週巡一次）：

1. 逐一開上表的官方來源，看下一年度是否已公布。證交所先以 `date=<西元年>` 查詢，回 0 筆就是未公布。
2. 只收週一至週五的休市日與半日市；名稱用台灣慣用譯名；每筆填 `src`，並在 `holidaySources` 補上（或沿用）出處與查證日期。
3. 該年度資料不完整時（如 HKEX 只公布到 10 月），`through` 填已公布的最後日期，**不要填 12-31**；資料待複核時加 `verified: false`。
4. 官方頁面沒給的不要靠第三方網站或記憶補；未公布就不收，並在本檔「待查證」註明。
5. 跑上面兩個驗證指令；有改動筆數時同步更新 `config-default.test.mjs` 的筆數表。
6. 使用者端：自己的 `wallpaper-config.json` 只需補新年度的 `holidays.<市場>.<年度>`（含 `through` 與 `days`），其餘由預設值補齊。
