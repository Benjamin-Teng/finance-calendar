# update_tw_events.py 行為規格（Python 版 → Rust 移植驗收清單）

> 依據：分支 `feat/data-layer-rust`，`D:\projects\finance-calendar-data-layer-rust\update_tw_events.py`（實際 1319 行，非 AGENTS.md 提到的約 985 行；多出的是動態桌布三鍵）。
> 方法：逐行讀程式；消費端以 grep 全 repo 後逐檔讀鍵。**以程式為準**，與 `AGENTS.md` 不一致處集中在第 9 節。
> 未實跑、未發任何網路請求。凡「Yahoo／TWSE／MOPS 實際回應形狀」的描述，來源是程式註解、`tests/fixtures/README.md` 與錄製樣本，標「樣本」者可對照 `tests/fixtures/`。
> 條目編號規則：`B-<區>-<序>`，驗收時可直接引用。行號一律指 `update_tw_events.py`，其他檔另寫路徑。

## 0. 總覽與編號區

| 區 | 內容 |
| --- | --- |
| B-OUT | 輸出檔格式（鍵結構） |
| B-HTTP | HTTP 層（共用） |
| B-DATE | 日期／時區共用工具 |
| B-FF | 總經（ForexFactory） |
| B-DIV | 除權息 |
| B-MTG | 股東會 |
| B-TOP | 市值前百大 |
| B-CONF | 法說會／財報（MOPS） |
| B-NEWS | 法說會備援（重大訊息） |
| B-PUN | 處置股 |
| B-QT | 行情（Yahoo） |
| B-SOFR | SOFR 3M |
| B-HOL | 休市日曆 |
| B-ID | 動態桌布：加權盤中走勢 |
| B-DK | 動態桌布：加權日 K |
| B-MG | 動態桌布：券資比與融資維持率 |
| B-GRD | 動態桌布鍵的共用護欄 |
| B-EVT | 事件合併、窗口、去重、counts |
| B-FLOW | 主流程、fetched、輸出目錄、結束碼、日誌 |
| C | 常數表 |
| U | 消費端（讀 `tw_events.json` 的程式與其鍵） |
| T | 測試與 fixture 現況 |
| K | 已知坑與移植風險 |

## 1. 輸出檔格式

### 1.1 檔案本體

- **B-OUT-1** 每輪產出兩個檔，內容同一份 payload：`tw_events.json`（`json.dumps(payload, ensure_ascii=False, indent=1)`，1 格縮排、非 ASCII 原樣）與 `tw_events.js`（`"window.TW_EVENTS = " + json.dumps(payload, ensure_ascii=False) + ";\n"`，單行、分隔符為預設的 `", "` 與 `": "`）。`update_tw_events.py:1284-1285`
- **B-OUT-2** 編碼 UTF-8、無 BOM。用 `Path.write_text(..., encoding="utf-8")` 寫，Windows 上文字模式會把 `\n` 轉成 `\r\n`（`.json` 的縮排換行與 `.js` 結尾換行都是 CRLF）。消費端（`JSON.parse`、serde_json、`<script>`）皆不受影響；Rust 版可任選 LF 或 CRLF，但**不可有 BOM**。`update_tw_events.py:1294-1295`
- **B-OUT-3** `tw_events.js` 的唯一消費者是 Lively 版 `finance-calendar.html`（`<script src>` 載入後讀 `window.TW_EVENTS`，失敗退讀 `.json`，`finance-calendar.html:830-841`）；宿主只讀 `.json`。Rust 版若不再服務 Lively，`.js` 可丟（見 B-FLOW-17）。
- **B-OUT-4** 浮點數用 Python `repr`（最短往返）輸出，整數值浮點帶 `.0`（如 `48000.0`）；極大／極小值 Python 寫 `1e+16`、`1e-05`。所有消費端皆為 JS／serde，兩種寫法都能讀；不要求位元級一致。`quotes[].prev` 這類值實際帶很長小數（樣本：`31.867900848388672`），**不可四捨五入**（見 B-QT-9）。

### 1.2 頂層鍵（鍵序＝ payload 建構順序，`update_tw_events.py:1242-1279`）

| 鍵 | 型別 | 何時存在 | 意義／範例 |
| --- | --- | --- | --- |
| `updated` | string `"YYYY-MM-DD HH:MM"` | 永遠 | **本機系統時區**（`datetime.now()`，非台北時區）的本輪寫檔時間，每輪必刷新，驅動前端重繪。例 `"2026-10-02 10:55"`。`:1241,1243` |
| `fetched` | string 同上 | 永遠 | 最後一次「真的抓到新資料」的時間。本輪 `any_fresh` 為真＝`now_str`；否則 `old.fetched`→`old.updated`→`now_str`。`:1245` |
| `window` | object `{start,end}` | 永遠 | 台北今日起 `[start,end]` ISO 日期（預設 today～today+14）。`:1246` |
| `counts` | object | 永遠 | 鍵固定 7 個：`dividend`、`meeting`、`conference`、`earnings`（四者＝過濾去重後 `events` 內各 type 的筆數）、`macro`（`len(macro)`，不受窗口影響）、`quotes`（含 SOFR）、`punish`。`:1235-1239` |
| `errors` | string[] | 永遠（無錯為 `[]`） | 既有來源的失敗訊息（中文字串）；前端只用 `.length`。見 B-FLOW-9。`:1248` |
| `wallpaper_errors` | string[] | 永遠（無錯為 `[]`） | 動態桌布三鍵的失敗訊息，**不進 `errors`**。`:1249` |
| `sources` | object | 永遠 | 純說明字串 11 筆：`macro`、`dividend`、`meeting`、`conference`、`earnings`、`punish`、`quotes`、`holidays`、`twii_intraday`、`twii_daily`、`margin`（文字照 `:1251-1261`）。無消費端讀取。 |
| `macro_meta` | object | 永遠 | 固定 `{"countries":"美國・歐元區・日本","importance":"中高重要性"}`（中點是 U+30FB）。`:1263` |
| `macro` | object[] | 永遠 | 總經事件，結構見 B-FF-8。 |
| `events` | object[] | 永遠 | 台股動態事件（dividend／meeting／conference／earnings），結構見 1.3。已套窗口、去重、排序。 |
| `punish` | object[] | 永遠 | 處置股，結構見 B-PUN-11。**不套窗口**。 |
| `quotes` | object[] | 永遠 | 行情條，結構見 B-QT-9／B-SOFR-5。 |
| `top100` | string[] | 永遠 | 市值前百大代號（4 位數字字串），每日重算一次的快取。可能為 `[]`。 |
| `top100_date` | string ISO 日期或 `null` | 永遠 | 上次**成功**算出名單的台北日期；沿用舊名單時必須保留舊值（B-TOP-6）；全新安裝又失敗＝`null`。 |
| `holidays` | string[] ISO 日期 | **條件**：本輪抓到、或舊檔有（`is not None`）才有；否則整鍵省略 | 當年度休市日，排序去重。樣本 `["2026-01-01","2026-02-12",...]`。`:1271-1272` |
| `twii_intraday` | object | **條件**：本輪成功或沿用舊值；全新安裝又失敗＝省略 | `{"date":"YYYY-MM-DD","points":[[epoch秒(int),價(float,2位)],...]}`。`:1274` |
| `twii_daily` | object[] | 同上 | `[{"date","open","high","low","close"},...]`，由舊到新，≥40 根（樣本 63 根）。 |
| `margin` | object | 同上 | `{"date","margin_lots","short_lots","short_margin_ratio","maintenance_ratio","unpriced":[代號...]}`。 |

- **B-OUT-5** 三個動態桌布鍵（`twii_intraday`／`twii_daily`／`margin`）加 `wallpaper_errors` 是動態桌布 change 新增；「既有鍵的格式與語意不變」。`wallpaper_errors` 在 payload 中**永遠存在**，三鍵可缺席。`update_tw_events.py:1273-1279`
- **B-OUT-6** 樣本實際鍵序：`updated, fetched, window, counts, errors, wallpaper_errors, sources, macro_meta, macro, events, punish, quotes, top100, top100_date, holidays, twii_intraday, twii_daily, margin`（`tests/fixtures/tw_events_sample.json`）。鍵序無消費端依賴，但建議保持以利 diff。

### 1.3 子結構

- **B-OUT-7** `macro[]` 元素（見 B-FF-8）：`dt`（`"YYYY-MM-DDTHH:MM"` 台北）、`date`、`time`、`ts`（int epoch 秒）、`country`（`US`/`EU`/`JP`）、`flag`（`美`/`歐`/`日`）、`impact`（`high`/`medium`）、`title`（中譯）、`title_en`、`forecast`、`previous`（皆字串、可為空字串）。範例：`{"dt":"2026-09-28T21:30","date":"2026-09-28","time":"21:30","ts":1790602200,"country":"EU","flag":"歐","impact":"medium","title":"歐洲央行總裁拉加德談話","title_en":"ECB President Lagarde Speaks","forecast":"","previous":""}`
- **B-OUT-8** `events[]` 元素：`{"date":"YYYY-MM-DD","type":"dividend|meeting|conference|earnings","code":str,"name":str,"note":str}`，鍵就這 5 個。範例：`{"date":"2026-10-05","type":"earnings","code":"2308","name":"台達電","note":"Q2 財報"}`、`{"date":"2026-10-05","type":"dividend","code":"00406A","name":"主動中信台灣收益","note":"除息 現金0.138元"}`、`{"date":"2026-10-02","type":"meeting","code":"3494","name":"誠研","note":"股東臨時會"}`。
- **B-OUT-9** `punish[]` 元素：`{"code":str,"name":str,"start":"YYYY-MM-DD","end":"YYYY-MM-DD","times":int,"market":"上市"|"上櫃"}`。範例 `{"code":"2030","name":"彰源","start":"2026-10-02","end":"2026-10-08","times":1,"market":"上市"}`。
- **B-OUT-10** `quotes[]` 元素：`{"name","kind","price","prev","chg_abs","chg_pct","t"}`；`kind` ∈ `fx|yield|cmdty|index|stock`；`t` 為 int epoch 秒或 `null`（Yahoo 無 `regularMarketTime`）。樣本 13 筆，順序＝`QUOTES` 順序，SOFR 3M 永遠最後。
- **B-OUT-11** 這份 payload 全為 JSON 可序列化的基本型別（`tests/test_wallpaper_data.py:709-719` 只驗新鍵）。

## 2. HTTP 共用層

- **B-HTTP-1** 所有請求帶固定 header `User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) finance-calendar/1.0`（Python 預設 UA 會被 Yahoo 擋）。`:119`、`:278`
- **B-HTTP-2** 逾時 `TIMEOUT = 30` 秒（單次 `urlopen`，含連線與讀取），**無重試、無退避**。重試責任在排程（每 6 小時）與沿用舊資料。`:95`、`:285`
- **B-HTTP-3** 預設 `urllib` 行為：跟隨 3xx 轉址；HTTP 4xx／5xx 丟 `HTTPError`（`URLError` 子類）；讀環境／系統代理（Windows 含登錄代理）；不送 `Accept-Encoding`。
- **B-HTTP-4** SSL 退回：先正常驗證；若例外是 `ssl.SSLError`，或是 `URLError` 且 `.reason` 是 `ssl.SSLError`，改用 `CERT_NONE`＋`check_hostname=False` 重打同一請求（**對所有主機**，不只 twse）。HTTPError 與純網路錯誤原樣往上丟。觸發情境＝新版 OpenSSL（Python 3.13）對 `twse.com.tw` 憑證鏈（AKI/SKI）更嚴。`:284-298`、AGENTS.md 現況「HTTPS 取用」。Rust 版：需決定是否保留此退回（用 rustls／native-tls 的行為與 OpenSSL 不同，須實測 twse 憑證）。
- **B-HTTP-5** `http_json(url)` ＝ `json.loads(bytes.decode("utf-8"))`：嚴格 UTF-8（壞位元組丟 `UnicodeDecodeError`）；開頭 BOM 會讓 `json.loads` 失敗。`:301-302`
- **B-HTTP-6** `http_bytes(url, form)`：`form` 非 `None` 時 POST，body＝`urllib.parse.urlencode(form)`（保持 dict 插入順序），`Content-Type: application/x-www-form-urlencoded`。MOPS 用。`:276-283`
- **B-HTTP-7** 失敗訊息直接用 `str(e)` 拼進 `errors`（見 B-FLOW-9 的字串清單）；訊息內文因語言／函式庫而異，無消費端解析（前端只數筆數）。
- **B-HTTP-8** 抓取層「可預期失敗」白名單 `FETCH_ERRORS`＝`OSError, ValueError, LookupError, TypeError, AttributeError, ArithmeticError, http.client.HTTPException`，只用於動態桌布三鍵（既有來源多用 `except Exception`）。`:162-163`

## 3. 日期與時區共用規則

- **B-DATE-1** `TPE = timezone(timedelta(hours=8))`，固定 +08:00（台北無夏令）。`:120`
- **B-DATE-2** 「今天」＝`datetime.now(TPE).date()`，**不用系統時區**（防排程 00:00 JST 把窗口推前一日）。`:1140`（main）、`:493`（top100）、`:530`（MOPS 月份）。
- **B-DATE-3** `updated`／`fetched` 例外：用 **`datetime.now()` 系統本機時區**（naive），格式 `%Y-%m-%d %H:%M`。前端以 `new Date(fetched.replace(' ','T'))` 當**本機時間**解讀（`host/ui/widgets/dynamic.js:156-160`），故 Rust 版必須寫本機時間而非 UTC／台北時間，否則人在其他時區時「已 N 天未更新」會偏。`:1241`
- **B-DATE-4** `roc_to_date(s)`：空值→`None`；`str(s).strip()` 後依序試兩個 regex（原文）：`^(\d{2,3})[年/.\-](\d{1,2})[月/.\-](\d{1,2})日?$`（支援 `115/07/15`、`115年07月15日`、`115.7.5`、`115-07-15`），不中再試 `^(\d{3})(\d{2})(\d{2})$`（`1150715`）。年＋1911；日期不合法（如 2 月 30）→`None`。8 位西元 `20261001` **不吃**（回 `None`）。`:327-341`
- **B-DATE-5** 7 碼民國日期另有兩處手寫解析（不經 `roc_to_date`）：holidays 的 `Date`（B-HOL-3）與處置期間（B-PUN-3）；二者都要求去 `/` 後恰好 7 個數字，否則整列略過。
- **B-DATE-6** 台北收盤時刻 `tpe_close_at(d)＝d 13:30 +08:00`；`hhmm_tpe(ts)＝fromtimestamp(ts, TPE).strftime("%H:%M")`。`:845-850`
- **B-DATE-7** 總經 `dt/date/time` 以 TPE 換算（B-FF-5）；前端再以 `ts`＋使用者系統時區重算顯示，只有舊資料（無 `ts`）才退回這些台北字串。

## 4. 來源規格

### 4.1 總經日曆（ForexFactory）`fetch_macro`，`:354-400`

- **B-FF-1** 端點：`https://nfs.faireconomy.media/ff_calendar_thisweek.json`、`.../ff_calendar_nextweek.json`，兩檔依序 GET，回應頂層為事件陣列（`http_json(url) or []`）。`:122-123`、`:359-361`
- **B-FF-2** 失敗語意：本週檔失敗＝`errors += "總經來源失敗（本週檔）：{e}"` 並**整個回傳 `None`**（呼叫端整鍵沿用舊 `macro`）；下週檔失敗（通常週末才發布，平日 404 屬正常）＝只 `log("[總經] 下週檔尚未發布…")`，**不進 errors**、不影響回傳。判別以 `url == URL_FF_NEXT`。`:363-371`
- **B-FF-3** 過濾：`country` ∈ `("USD","EUR","JPY")` 且 `impact` ∈ `("High","Medium")`（區分大小寫）。**不套 14 天窗口**（總經涵蓋整個本週＋下週，樣本最早一筆比 `window.start` 還早）。`:92-93`、`:376-377`
- **B-FF-4** 日期：`datetime.fromisoformat(r["date"]).astimezone(TPE)`；解析失敗（含缺鍵）的該列**靜默略過**（`continue`）。FF 的 `date` 帶偏移（如 `2026-09-28T08:30:00-04:00`）；若來源給 naive 時間，Python 會當**系統本機時區**（Rust 版須決定，預期不會發生）。`:378-381`
- **B-FF-5** 輸出時間欄：`dt="%Y-%m-%dT%H:%M"`、`date="%Y-%m-%d"`、`time="%H:%M"`（均台北）、`ts=int(dt.timestamp())`（截斷小數，秒）。`:388-391`
- **B-FF-6** 去重：以**原始** `(r.get("title"), r.get("date"))` 字串元組，先到先留。`:382-385`
- **B-FF-7** 排序：依 `dt` 字串升冪，**穩定排序**（同 `dt` 保持來源順序，且 thisweek 先於 nextweek）。`:399`
- **B-FF-8** 欄位對照：`country`/`flag` 來自 `CCY`（USD→`US`/`美`、EUR→`EU`/`歐`、JPY→`JP`/`日`；字典另含 CNY→`CN`/`中`、GBP→`GB`/`英`，因過濾而不會用到）；`impact` ＝ `"high"`（High）或 `"medium"`；`title`＝`zh_title(r.get("title",""))`；`title_en`＝原標題；`forecast`／`previous`＝`(r.get(..) or "").strip()`。`:165-166`、`:386-398`
- **B-FF-9** 中譯 `zh_title(t)`：先查字典 `T`（94 條，`:169-256`，精確字串比對）；沒對到則依序全域取代 `" m/m"→" (月增)"`、`" y/y"→" (年增)"`、`" q/q"→" (季增)"`、`" Speaks"→" 談話"`、`" Testifies"→" 聽證"`；再沒有＝保留英文。範例：`"Foo y/y Speaks"` → `"Foo (年增) 談話"`。`:259-266`
- **B-FF-10** 潛在脆弱點：`fetch_macro` 的列轉換迴圈（`:373-398`）只有 `fromisoformat` 在 try 內；`forecast`／`previous` 若非字串（數字）→`.strip()` 丟 `AttributeError`，**沒有任何 try 接住，會一路穿出 `main()` 使整輪不寫檔**（見 K-14）。實務上 FF 皆回字串。
- **B-FF-11** 失敗沿用：`macro is None` → `macro = old.get("macro") or []`；`any_fresh` 以「回傳非 None」判定（`macro==[]` 也算新）。`:1150-1154`

### 4.2 除權息 `fetch_dividend`，`:405-433`

- **B-DIV-1** 主端點 `https://www.twse.com.tw/rwd/zh/exRight/TWT48U?response=json`；備援 `https://openapi.twse.com.tw/v1/exchangeReport/TWT48U_ALL`。`:124-125`
- **B-DIV-2** 主端點解析：`j.get("data") or []`，每列為陣列：`r[0]`＝日期（`roc_to_date`）、`r[1]`＝代號、`r[2]`＝名稱、`r[3]`＝權息別、`r[7]`＝現金股利。日期解析失敗的列略過。輸出 `{"date":ISO,"type":"dividend","code":str(r[1]).strip(),"name":str(r[2]).strip(),"note":"除"+(str(r[3]).strip() or "權息")+fmt_cash(r[7])}`。主端點成功（即使 data 為空）直接 `return ev`，**不會**走備援。`:409-417`
- **B-DIV-3** 備援解析：`http_json(URL_DIV_OAPI)` 為物件陣列，欄位 `Date`（`roc_to_date`）、`Code`、`Name`、`Exdividend`（`or "權息"`，**未 strip**）、`CashDividend`。note＝`"除"+Exdividend+fmt_cash(CashDividend)`。`:421-430`
- **B-DIV-4** `fmt_cash(cash)`：`float(str(cash).strip())`，值 `>0` → `f" 現金{v:g}元"`（前導一個空白；Python `%g`＝6 位有效數字、整數不帶小數點、指數門檻 <1e-4 或 ≥1e6 改科學記號，如 `0.138`→`0.138`、`100.0`→`100`、`1.23456789`→`1.23457`）；`≤0` 或解析失敗→空字串。`:344-349`
- **B-DIV-5** 兩來源皆失敗：`errors += "除權息來源失敗：{e}"`，回傳 `None`→呼叫端沿用 `old.events` 中 `type=="dividend"` 的項目。主端點失敗只記 log（`[除權息] 即時 API 失敗…`），**不進 errors**。`:418-419`、`:431-433`、`:1160-1164`
- **B-DIV-6** 已知怪癖：`ev` 在主端點失敗後**沒有清空**就接著備援。若主端點在處理到第 N 列時才丟例外（例如某列少於 8 欄），前 N−1 列留在 `ev`，備援再追加完整資料→先有重複列，之後由 B-EVT-3 的去重吃掉（同 type＋code＋date）。Rust 若改成「主端點失敗則丟棄部分結果」，行為差異僅在極端情況，需決定是否保留。`:406-431`

### 4.3 股東會 `fetch_meeting`，`:436-455`

- **B-MTG-1** 端點 `https://openapi.twse.com.tw/v1/opendata/t187ap41_L`，物件陣列；欄位（中文鍵）：`開會日期`（`roc_to_date`）、`股東常(臨時)會`、`是否改選董監`、`公司代號`、`公司名稱`。`:126`、`:440-451`
- **B-MTG-2** `note`：`kind` 非空→`"股東"+kind`（如 `股東常會`、`股東臨時會`），空→`"股東會"`；`是否改選董監`（strip 後）`=="是"`→再加 `"・改選董監"`。`type="meeting"`。`:444-451`
- **B-MTG-3** 日期解析失敗的列略過；來源失敗 `errors += "股東會來源失敗：{e}"`，回傳 `None`，沿用 `old.events` 的 `type=="meeting"`。**不套窗口於本函式**（窗口在 B-EVT-3）。`:452-454`、`:1165-1169`

### 4.4 市值前百大 `fetch_top100`，`:489-518`

- **B-TOP-1** 端點：`https://openapi.twse.com.tw/v1/opendata/t187ap03_L`（股數，約 1.3 MB）＋`https://openapi.twse.com.tw/v1/exchangeReport/STOCK_DAY_ALL`（收盤價，約 0.3 MB）。`:131-132`
- **B-TOP-2** 每日只算一次：`today=台北日期 ISO`；`cached=old.top100 or []`；`cached 非空 且 old.top100_date==today`→直接回傳 cached，**不發任何請求**。`:493-496`
- **B-TOP-3** 價格表：`STOCK_DAY_ALL` 每列 `Code`（strip）、`ClosingPrice`（`str(... or "")` 去逗號 strip 後 `float`，`>0` 才收；解析失敗列略過）。`:498-505`
- **B-TOP-4** 市值：`t187ap03_L` 每列 `公司代號`（strip）、`已發行普通股數或TDR原股發行股數`（strip，須 `.isdigit()`）；`code in price and shares.isdigit()` 才算 `(int(shares) * price[code], code)`。`:506-511`
- **B-TOP-5** 排序＝`caps.sort(reverse=True)`：先依市值降冪，**市值相同時代號字串降冪**；取前 `TOP_N=100` 的代號（字串，保持此順序）。`caps` 空→`ValueError("市值一家都算不出來…")`→視同失敗。任何例外→`errors += "市值前百大來源失敗：{e}"` 回傳 `None`。`:512-518`
- **B-TOP-6** main 層：`from_cache = bool(old.top100) and old.top100_date==today`；回傳 `None`→沿用 `old.top100 or []` 且 **`top100_date` 必須保留 `old.top100_date`**（否則被當成今天已算過、整天不重試；全新安裝為 `None`）；成功→`top100_date=today`，且 `any_fresh` 只在**非快取**時為真（走快取沒發請求，不算抓到新資料）。`:1170-1179`
- **B-TOP-7** 此名單會被 B-CONF 當白名單：`top100` 為空（含全新安裝 top100 失敗）→B-CONF-1。

### 4.5 法說會／財報（MOPS）`fetch_conference`，`:521-559`

- **B-CONF-1** `top100` 為空→`errors += "法說會：無市值前百大名單可篩，本輪改用備援來源"`，回傳 `None`（理由：「只收前百大」在名單空時等於全不收，靜默空清單會被誤讀為「近兩週沒有財報」）。`:526-528`
- **B-CONF-2** 端點 `POST https://mopsov.twse.com.tw/mops/web/ajax_t100sb02_1`（舊站 mopsov 才吃）。表單（依序）：`encodeURIComponent=1`、`step=1`、`firstin=1`、`off=1`、`TYPEK=sii`、`year=<民國年（西元−1911，不補零）>`、`month=<MM 兩位>`。**抓本月與下月兩次**，月份算法 `mo=today.month+k; y=today.year+(mo-1)//12; mo=(mo-1)%12+1`（跨年進位）。回應 `decode("utf-8","replace")`。`:130`、`:533-539`
- **B-CONF-3** 上櫃（`TYPEK=otc`）**不抓**（實測前百大無上櫃）。`:29`
- **B-CONF-4** HTML 解析（regex 原文，全部 `re.S|re.I` 除 TAG 外；`RE_TAG` 無旗標）：
  - `RE_TR = <tr[^>]*>(.*?)</tr>`（S|I）
  - `RE_TD = <t[dh][^>]*>(.*?)</t[dh]>`（S|I）
  - `RE_TAG = <[^>]+>`
  - `RE_CODE = ^\d{4}$`
  每個 `<tr>` 取所有 `td/th` 內文，**去標籤→`html.unescape`→strip**；只保留 `cells[0]` 符合 `RE_CODE`（恰 4 位數字）的列。`:458-475`
- **B-CONF-5** 欄位位置：`r[0]`＝代號、`r[1]`＝名稱、`r[2]`＝日期欄、`r[4]`＝地點、`r[5]`＝擇要訊息；`len(r)<6` 略過；`r[0]` 不在 `top100` 集合略過。`:540-548`
- **B-CONF-6** 日期欄兩種格式：單日 `115/07/16`、區間 `115/06/30 至 115/07/03`（實測 105 筆中 8 筆）；**一律取起日**＝`roc_to_date(r[2].split("至")[0].strip())`；解析失敗略過。`:544-547`
- **B-CONF-7** 季別 `quarter_of(brief)`（regex 原文）：先 `RE_Q_ZH = 第\s*([一二三四1-4])\s*季`（`ZH_NUM` 對照 一/1→1、二/2→2、三/3→3、四/4→4），不中再 `RE_Q_EN = \b([1-4])Q\d{2}\b|\bQ([1-4])\b`（`re.I`）；取 `group(1) or group(2)`。無旗標下 Python `\b`、`\d` 皆 **Unicode 感知**（見 K-5：「公布2Q26財報」這種緊貼中文的寫法**不會**命中 EN 規則）。`:463-486`
- **B-CONF-8** 輸出：`q` 有值→`type="earnings"`、`note=f"Q{q} 財報"`；否則 `type="conference"`、`note="法說會"`。`"線上" in place or "線上" in brief`→`note += "（線上）"`。`name=r[1]`、`code=r[0]`。`:549-555`
- **B-CONF-9** 失敗粒度：兩個月份任一個請求／解析丟例外→整個函式回傳 `None`（**不保留已抓到的第一個月**），`errors += "法說會來源（MOPS）失敗：{e}"`。`:556-558`
- **B-CONF-10** main 退回鏈：`fetch_conference`→`None`→`fetch_conference_news`→仍 `None`→沿用 `old.events` 的 `conference`＋`earnings`。`any_fresh` 只要最終 `conf_ev is not None`（含備援成功）即真。**注意**：備援成功時 `earnings` 型別事件會消失（備援只產 `conference`），且 `errors` 仍留有 MOPS 失敗訊息。`:1181-1189`

### 4.6 法說會備援（重大訊息）`fetch_conference_news`，`:562-589`

- **B-NEWS-1** 端點 `https://openapi.twse.com.tw/v1/opendata/t187ap04_L`（只有**當日**公告、抓不到未來場次）。`:127`
- **B-NEWS-2** 欄位：`符合條款`、`主旨<尾端空白>`（鍵名帶尾端空白，即 "主旨" 後接一個空格）或 `主旨`、`說明`、`事實發生日`、`公司代號`、`公司名稱`。`subj=str(r.get("主旨 ") or r.get("主旨") or "")`。`:568-570`
- **B-NEWS-3** 篩選：跳過條件①`not re.search(r"第\s*12\s*款", clause) and "法人說明會" not in subj`；跳過條件②`"法人說明會" not in subj and "法人說明會" not in body`。`:571-574`
- **B-NEWS-4** 日期：先在 `說明` 以 `召開法人說明會之日期[：:]\s*(\d{2,3}/\d{1,2}/\d{1,2})` 取；沒有則 `roc_to_date(事實發生日)`；失敗略過。`:575-577`
- **B-NEWS-5** 輸出 `type="conference"`、`note="法說會"`＋（`"線上" in subj or "線上" in body[:200]`→`（線上）`）；**不篩前百大**、不產 `earnings`。失敗 `errors += "法說會備援來源（重大訊息）失敗：{e}"`→`None`。`:578-588`

### 4.7 處置股 `fetch_punish`，`:638-701`

- **B-PUN-1** 端點：上市 `https://openapi.twse.com.tw/v1/announcement/punish`（欄位 `Code`、`Name`、`DispositionPeriod`、`NumberOfAnnouncement`）；上櫃 `https://www.tpex.org.tw/openapi/v1/tpex_disposal_information`（欄位 `SecuritiesCompanyCode`、`CompanyName`、`DispositionPeriod`、`NumberOfAnnouncement`）。`:133-134`、`:651-677`
- **B-PUN-2** 代號過濾（只收股票／ETF，濾權證、可轉債）：`RE_STOCK_CODE=^\d{4}[A-Z]?$`（含特別股 `2891B`）或 `RE_ETF_CODE=^00\d{2,4}$`（`0050`、`006208`）。代號先 `str(...).strip()`。`:592-598`
- **B-PUN-3** 期間雙格式 `parse_disposition_period`：`str(s).strip()`；含全形 `～` 以 `split("～",1)`（TWSE，如 `115/07/03～115/07/16`），否則含半形 `~`（TPEx，如 `1150710~1150723`），否則 `None`；兩段各 `strip().replace("/","")` 須恰 7 位數字，年＋1911 組日期（不合法→`None`）。任何失敗→該列**靜默跳過**，不記錯誤。`:601-627`
- **B-PUN-4** TPEx 空資料時回「**單筆全空白樣板列**」而非空陣列→代號過濾即被擋掉（代號空字串不符合 regex），不算錯誤；TPEx openapi 不吃日期參數（固定 snapshot）。AGENTS.md 現況。
- **B-PUN-5** `times=punish_times(NumberOfAnnouncement)`：`int(str(raw).strip())`，失敗（含 TPEx 無此欄位）一律 `1`。`:630-635`
- **B-PUN-6** 兩來源**各自獨立**成功旗標（`twse_ok`、`tpex_ok`）；失敗各自 `errors += "處置股來源失敗（上市）：{e}"` ／ `"…（上櫃）…"`，不阻斷對方。`http_json(...) or []`。`:648-680`
- **B-PUN-7** 失敗沿用（**按 market 分段**）：`twse_ok` 為假→把 `old_punish` 中 `market=="上市"` 的列併入；`tpex_ok` 為假→併入 `market=="上櫃"` 的列；有沿用才 log `[處置股] 上市沿用上次資料（N 筆）`。`old_punish=old.get("punish")`。`:682-691`
- **B-PUN-8** 二度處置：來源會「第一次＋第二次」兩筆公告並存（期間重疊）；同 `code` **只保留 `(end, start)` 最大**那筆（字串比較 ISO，故等價於日期比較）；`times` 取保留筆自己的 `NumberOfAnnouncement`（來源會把同檔所有列都更新成累計值）。去重跨市場以 `code` 為鍵（不含 market）。`:693-700`
- **B-PUN-9** 輸出按 `code` 字串升冪排序；**不套 14 天窗口**（消費端自己以「目標日落在 start～end」過濾）。`:701`
- **B-PUN-10** `fetch_punish` 永不回 `None`；`any_fresh` 不計 punish。`:1192`
- **B-PUN-11** 輸出元素 `{"code","name","start","end","times","market"}`，`market` 取 `"上市"`／`"上櫃"`。`:657-660`、`:674-677`

### 4.8 行情條 `fetch_quotes`，`:732-768`（清單見 C 區）

- **B-QT-1** 端點樣板 `https://query1.finance.yahoo.com/v8/finance/chart/{symbol}?interval=1d&range=5d`，`symbol` 先 `urllib.parse.quote(symbol)`（預設 safe=`/`，故 `^`→`%5E`、`=`→`%3D`；`USDTWD=X`→`USDTWD%3DX`、`CL=F`→`CL%3DF`、`^TWII`→`%5ETWII`）。`:135`、`:739`
- **B-QT-2** 取值：`result=(j.chart).result`；空→`ValueError("result 為空（{chart.error}）")`。`meta=result[0].meta`；`price=meta.regularMarketPrice`（v8 沒有 `previousClose` 鍵）；`t=meta.regularMarketTime`。`:741-745`、`:759`
- **B-QT-3** 昨收＝`prev_close_from_chart(result[0]) or meta.get("chartPreviousClose")`。**絕不可只用 `chartPreviousClose`**（它是所請求 range 起點之前的收盤，`range=5d` 下＝數個交易日前，漲跌幅變多日累計；2026-07-31 實測加權 +7.98% 顯示成 −1.18%）。`range=5d` 維持不變（要留窗口給連假／停市）。`:706-729`、`:748`
- **B-QT-4** `prev_close_from_chart` 演算法（逐步）：
  1. `closes=indicators.quote[0].close or []`；`stamps=result.timestamp or []`。
  2. 若 `stamps` 非空且 `len(stamps)==len(closes)`：`offset=meta.gmtoffset or 0`；`rows=[((t+offset)//86400, float(c)) for t,c in zip(stamps,closes) if c is not None]`（把每筆換成「交易所當地日序」，整數除法）；若 `rows` 非空：`today=rows[-1][0]`；回傳「由後往前第一個日序 ≠ today 的 close」，找不到→回傳 `None`（此時**直接回 `None`，不走第 3 步**，由呼叫端退回 `chartPreviousClose`）。
  3. 否則（無 timestamp、長度不符、或 `rows` 為空）：`valid=[float(c) for c in closes if c is not None]`；`len(valid)>=2` 回 `valid[-2]`，否則 `None`。
  理由：盤中 Yahoo 會在當日 K 之外**另附一筆即時報價**（USDTWD=X 實測：當日 K 的 close 為 null，另有一筆 08:58 即時值），同一交易日佔兩個位置，直接取倒數第二筆會拿到當日自己；各檔 K 皆落在當地開盤整點，離換日點夠遠，故用 gmtoffset 歸戶是安全的。`:718-729`
- **B-QT-5** `price is None`→`ValueError("regularMarketPrice 缺值")`；`prev` 為 `None` 或 `0`（`not prev`）→`ValueError("前一交易日收盤缺值或為 0")`。注意 `prev_close_from_chart(...) or chartPreviousClose` 使得前者回 `0.0` 也會退回後者。`:749-752`
- **B-QT-6** 計算：`price,prev=float(),float()`；`chg_abs=price-prev`；`chg_pct=(price/prev-1)*100`（**不捨入**）。`:753-758`
- **B-QT-7** 逐檔失敗語意：訊息 `行情來源失敗（{name}／{symbol}）：{e}` 同時 `log("[行情] …，跳過")` 並 **append 進 `errors`**；若 `old_quotes`（以 `name` 為鍵的舊 `quotes`）有同名項目，**原樣沿用該舊項**（log `[行情] {name} 沿用上次資料`），否則該檔省略。沿用項保持在清單原位置。`:761-767`
- **B-QT-8** `old_quotes` 由 `{q.name: q for q in old.quotes if q.name}` 組成（舊檔裡也含 SOFR 3M 項目）。`:1194`
- **B-QT-9** 輸出元素 `{"name","kind","price","prev","chg_abs","chg_pct","t"}`；`prev` 為完整浮點（不捨入）。`:754-760`
- **B-QT-10** `any_fresh` **不**因 Yahoo 行情抓到而為真（只看 SOFR）；`counts.quotes` 為最終 `quotes` 長度（含沿用、含 SOFR）。`:1194-1202`、`:1238`

### 4.9 SOFR 3M `fetch_sofr`，`:771-801`

- **B-SOFR-1** 端點 `https://markets.newyorkfed.org/api/rates/secured/sofrai/last/2.json`；有效筆數不足 2 時改抓 `.../last/5.json`。回應 `refRates[]`，欄位 `effectiveDate`（ISO 日期字串）、`average90day`（90 天複合平均）。`:139-140`
- **B-SOFR-2** 有效筆＝`effectiveDate` 真值且 `average90day is not None`；過濾後 `<2` 筆才打 fallback；兩次都不足→`ValueError("有效資料不足兩筆")`。`:776-785`
- **B-SOFR-3** 以 `effectiveDate` 字串**降冪**排序，`latest=rows[0]`、`prev=rows[1]`。`:783-786`
- **B-SOFR-4** `price=float(latest.average90day)`、`prev=float(prev.average90day)`；`t=` `effectiveDate` 當日 **UTC 00:00** 的 epoch 秒（`datetime(y,m,d,tzinfo=utc).timestamp()` 轉 int）。`:787-789`
- **B-SOFR-5** 輸出 `{"name":"SOFR 3M","kind":"yield","price","prev","chg_abs","chg_pct","t"}`，`chg_pct=(price/prev-1)*100`。`:790-796`
- **B-SOFR-6** 失敗：log＋`errors += "行情來源失敗（SOFR 3M／NY Fed）：{e}"`→回傳 `None`；main 沿用 `old_quotes["SOFR 3M"]`（有才用）；有 sofr 才 `quotes.append(sofr)`（永遠在最後）。`any_fresh` 以「**fetch_sofr 本身**回傳非 None」計（沿用舊值不算）。`:797-801`、`:1196-1202`

### 4.10 休市日曆 `fetch_holidays`，`:804-831`

- **B-HOL-1** 端點 `https://openapi.twse.com.tw/v1/holidaySchedule/holidaySchedule`；**只回當年度**，跨年由前端週末規則兜底。`:136`
- **B-HOL-2** 失敗（`http_json` 例外）→log、`errors += "休市日曆來源失敗：{e}"`、回傳 `None`；main：`holidays is None and old.get("holidays") is not None`→沿用舊值；否則該鍵省略。`any_fresh` 以「回傳非 None」計（空清單也算）。`:807-812`、`:1204-1208`
- **B-HOL-3** 每列欄位 `Name`、`Date`、`Description`；`Date`＝民國 7 碼（`strip` 後須 `len==7 and isdigit`，否則略過），轉 `date(y+1911,m,d).isoformat()`，不合法略過；結果 `sorted(set(...))`。`:814-831`
- **B-HOL-4** **排除交易日說明列**（API 混入「國曆新年開始交易日」「農曆春節前最後交易日」等，那些日子照常開盤）：`TRADING_DAY_NOTICE = (開始|最後)交易`（`:138`）在 `Name` 中 `search` 命中，**且** `Description` 以 `"。"` 切後**第一句**不含 `"無交易"` → 跳過。只看第一句：2023-01-17（交易日）的說明第二句提到 1/18、1/19 無交易，看整段會誤判。`:821-823`
- **B-HOL-5** 邊界案例（2021／2022 年）：結算交割日的 `Name` 也寫「農曆春節前最後交易日」，但 `Description` 第一句寫「市場無交易，僅辦理結算交割作業」→這種是**真休市要保留**。測試涵蓋 2021–2026（`tests/test_wallpaper_data.py:627-706`）。memory：`twse-holidayschedule-includes-trading-days.md`。

### 4.11 動態桌布鍵：加權盤中走勢 `fetch_twii_intraday`／`parse_twii_intraday`，`:853-909`

- **B-ID-1** 端點 `https://query1.finance.yahoo.com/v8/finance/chart/%5ETWII?interval=5m&range=5d`（5m 資料 Yahoo 只提供 60 天內；`range=5d`＝最近 5 個**交易日**，連假也撈得到上一個完整日；查證見 `tests/fixtures/README.md`）。`:145`
- **B-ID-2** 解析：`stamps=timestamp or []`，`closes=indicators.quote[0].close or []`；`stamps` 空或長度≠closes→`ValueError`。逐筆：`close is None` 或 `t % 300 != 0`（不在 5 分鐘格線，如 Yahoo 另附的即時報價）→丟棄；`d=fromtimestamp(t,TPE)`；`MARKET_OPEN(09:00) <= d.time() <= MARKET_CLOSE(13:30)` 才收（**13:30:00 整點的點保留**）；`days[d.date()][int(t)] = round(float(c), 2)`（同 ts 後者蓋前者）。`:864-876`
- **B-ID-3** 「最後一個完整交易日」：`done=[d for d in days if tpe_close_at(d) <= now]`（`now` 轉 TPE；**盤中執行時當日不合格**，取前一日）；空→`ValueError("序列內沒有任何已過 13:30 的交易日")`；`day=max(done)`；`points=sorted(days[day].items())`。`:877-881`
- **B-ID-4** 完整性檢查，任一不過→`ValueError`（視同失敗、不拿不完整序列冒充）：起點 `first > 當日09:00 + 300s`（即晚於 09:05）；終點 `last < 當日13:30 − 300s`（即早於 13:25，來源尚未追上收盤）；相鄰點間隔 `> 300` 秒。`:882-890`
- **B-ID-5** 輸出 `{"date":day.isoformat(),"points":[[t,p],...]}`，`t` 為 int 秒、`p` 為 2 位小數浮點；末筆是 13:25 那根，**不含 13:30 收盤試撮**（與官方收盤價不同）。`:891`
- **B-ID-6** `fetch_twii_intraday(wallpaper_errors, old, now)`：`old` 非 dict 一律當 `None`；`parse` 丟 `FETCH_ERRORS`→`wallpaper_errors += "加權盤中走勢來源失敗：{e}"`、log、**回傳 old**；成功但 `str(old.date or "") > new.date`（日期倒退）→**靜默回傳 old**（無錯誤訊息）；否則回傳 new。`:894-909`

### 4.12 動態桌布鍵：加權日 K `fetch_twii_daily`／`parse_twii_daily`，`:912-957`

- **B-DK-1** 端點 `https://query1.finance.yahoo.com/v8/finance/chart/%5ETWII?interval=1d&range=3mo`（樣本 65 筆原始列）。`:146`
- **B-DK-2** 解析：需 `timestamp` 與 `open/high/low/close` 四欄長度一致，否則 `ValueError`；任一 OHLC 為 `None` 的列丟棄（Yahoo 另附的即時列常如此）；`d=fromtimestamp(t,TPE).date()`；`tpe_close_at(d) > now`（當日 13:30 前）→丟棄該日；同日重複取**後者**（dict 覆蓋）；每個值 `round(float(x),2)`；依日期升冪輸出 `{"date","open","high","low","close"}`。保留整個 3 個月窗口（樣本 63 根），**不截成 40 根**。`:918-932`
- **B-DK-3** `len(out) < DAILY_MIN(40)`→`ValueError("日 K 只有 N 根，不足 40 根")`。`:933-935`
- **B-DK-4** `fetch_twii_daily`：`old` 非 list 當 `None`；`FETCH_ERRORS`→`wallpaper_errors += "加權日K來源失敗：{e}"`＋回傳 old；成功但 `old[-1].date > new[-1].date`（Yahoo 偶發回較舊視窗）→`wallpaper_errors += "加權日K來源回傳較舊的資料（最後一根 X 早於上次的 Y），沿用上次資料"`＋回傳 old（**與盤中走勢不同：這裡有記錯誤**）；否則 new。`:938-957`

### 4.13 動態桌布鍵：券資比與融資維持率 `fetch_margin`，`:960-1082`

- **B-MG-1** 端點樣板 `https://www.twse.com.tw/rwd/zh/marginTrading/MI_MARGN?selectType={kind}&response=json`；`kind=MS` 取彙總（不帶 `date`＝最新已公布日）；`kind=ALL` 後加 `&date=YYYYMMDD` 取個股明細。非交易日／尚未公布回 `{"stat":"很抱歉，沒有符合條件的資料"}`。`:149`、`:1065-1071`
- **B-MG-2** `margin_ok`：`stat != "OK"`→`ValueError`。回應頂層 `date`＝`YYYYMMDD` 西元（`ymd8_to_date`，正則 `(\d{4})(\d{2})(\d{2})` fullmatch，不符 `ValueError`）。`:964-975`
- **B-MG-3** `parse_margin_summary`：取 `tables[0]`；`fields.index("今日餘額")`（第一次出現）；`by_label={str(r[0]).strip(): r}`；取列標籤 `"融資(交易單位)"`、`"融券(交易單位)"`、`"融資金額(仟元)"` 的「今日餘額」欄；數字 `margin_int`＝去逗號 strip 後 `int`。回傳 `{date, margin_lots, short_lots, margin_amount_k}`（單位：張、張、仟元）。採「今日餘額」而非「前日餘額」（後者要等下一交易日）。`:978-992`
- **B-MG-4** `parse_margin_stocks`：掃 `tables`，找 `fields[:1]==["代號"]` 且 `"今日餘額"` 欄**恰兩個**的表；第一個＝融資、第二個＝融券；回傳 `(date_iso, [(代號 strip, 融資張, 融券張)...])`；找不到→`ValueError`。`:995-1006`
- **B-MG-5** `parse_day_all_prices`：全表 `Date`（`roc_to_date`）須為**單一有效日期**，否則 `ValueError`；`ClosingPrice` 去逗號 strip 後 `float`，非數字（空字串）／`≤0` 不收。回傳 `(day_iso, {code: price})`。`:1009-1024`
- **B-MG-6** `compute_margin`：`margin_lots<=0 or margin_amount_k*1000<=0`→`ValueError`；個股融資張數加總與彙總差距 `> 1%`（`MARGIN_SUM_TOLERANCE=0.01`）→`ValueError`（防欄位位移）；`value=Σ lots*1000*price`（查無價格者記入 `unpriced`，保持明細表順序）；`short_margin_ratio=round(short_lots/margin_lots*100, 2)`；`maintenance_ratio=round(value/(margin_amount_k*1000)*100, 2)`；ETF 與一般股票同樣計入。`:1027-1052`
- **B-MG-7** 流程 `fetch_margin(wallpaper_errors, old)`：①取 MS 彙總→交易日 D；`old.date==D`→直接回傳 old（**不再下載大表**）；②取 ALL(date=D)；`all_day != D`→`ValueError`；③取 `STOCK_DAY_ALL`（**與 top100 那次是另一次請求，每輪各下載一次**）；`price_day != D`→log `[券資比] 收盤價日期 … 不混用，沿用上次資料`、**回傳 old 且不記錯誤**（晚間公布前屬正常）；④`compute_margin`。`FETCH_ERRORS`→`wallpaper_errors += "券資比／融資維持率來源失敗：{e}"`＋回傳 old。`old` 非 dict 當 `None`。`:1055-1082`
- **B-MG-8** 由於 ③ 價格日≠D 時回 old；若同時 `old is None`（全新安裝）→回傳 `None`→鍵省略（測試 `test_fetch_mismatch_without_old_returns_none`）。

### 4.14 動態桌布三鍵共用護欄 `guarded_wallpaper_fetch`，`:1085-1095`

- **B-GRD-1** 三個 fetch 外再包一層 `except Exception`：任何非預期例外→`wallpaper_errors += "{label}來源發生非預期錯誤：{類名}: {e}"`、log、回傳 `old if isinstance(old, old_type) else None`。label 分別為 `加權盤中走勢`（old_type=dict）、`加權日K`（list）、`券資比／融資維持率`（dict）。`:1085-1095`、`:1215-1220`
- **B-GRD-2** 三鍵不計入 `any_fresh`（否則只要 Yahoo 通，`fetched` 每輪刷新、蓋掉「已 N 天未更新」）；錯誤不進 `errors`（Lively 頁腳以 `errors` 筆數顯示「⚠ N 個來源異常」）。`:1210-1212`
- **B-GRD-3** 抓取順序：intraday→daily→margin；`now`（測試注入）僅供這三鍵使用，`now_tpe = now or datetime.now(TPE)`。`:1214-1220`

## 5. 事件合併、窗口、去重、counts（B-EVT）

- **B-EVT-1** 合併順序：`raw = div_ev + meeting_ev + conf_ev`（`conf_ev` 內 earnings/conference 依 MOPS 列順序）。`:1190`
- **B-EVT-2** 窗口：`start = today - INCLUDE_PAST_DAYS(0)`，`end = today + WINDOW_DAYS(14)`；事件 `date.fromisoformat(e["date"])` 須 `start <= d <= end`（閉區間）。沿用的舊事件也經此過濾（可能因日期過期而掉出）。`:1141-1142`、`:1224-1227`
- **B-EVT-3** 去重鍵 `(type, code, date)`，先到先留（來源序＝div→meeting→conf）。`:1228-1232`
- **B-EVT-4** 排序鍵 `(date, type, code)`，三者皆字串升冪；`type` 的字母序＝`conference < dividend < earnings < meeting`（前端另依 `dynTypes` 索引重排）。`:1233`
- **B-EVT-5** `counts` 見 B-OUT 表；`counts.macro=len(macro)`、`counts.quotes=len(quotes)`、`counts.punish=len(punish)`。`:1235-1239`

## 6. 主流程

### 6.1 步驟順序（`main()`，`:1116-1315`）

- **B-FLOW-1** stdout 若是 `io.TextIOWrapper` 則 `reconfigure(encoding="utf-8", errors="replace")`（吞例外）；`log()` 對 `UnicodeEncodeError` 再退化列印。`:1117-1121`、`:269-273`
- **B-FLOW-2** **啟動等網路** `wait_for_network()`：用原始 socket（刻意不吃代理環境變數）`socket.create_connection(("www.twse.com.tw", 443), timeout=5)` 探測；成功即返回（`waited>0` 才 log `[網路] 已就緒（等待 N 秒）`）；失敗時若 `waited >= max_wait(300)`→log `[網路] 等待逾時…`、回傳 `False`；否則 log `尚未就緒…20 秒後重試`、`sleep(20)`、`waited += 20`。（`waited` 只累計 sleep，不含各次 5 秒連線逾時；最壞約 15 次重試、牆鐘 >300 秒。）逾時只放行不中止；回傳 `False`→`errors += "啟動時網路等待逾時，改用既有資料兜底"`（此字串會留在輸出 `errors`）。參數預設 `interval=20, max_wait=300, timeout=5`。`:305-323`、`:1127-1128`
- **B-FLOW-3** 讀舊輸出：`Path(__file__).resolve().parent / "tw_events.json"`（**只讀腳本所在資料夾那份，不讀 argv 額外目錄／Lively 目錄**），不存在或壞掉＝`{}`。`:1130-1136`
- **B-FLOW-4** 日期窗口以 `datetime.now(TPE).date()` 計（B-DATE-2）。log `抓取總經日曆＋台股動態事件＋行情＋休市日曆（{start} ~ {end}）…`。`:1140-1144`
- **B-FLOW-5** 抓取順序（**序列，非並行**）：macro → dividend → meeting → top100 → conference（含備援）→ punish → quotes → SOFR → holidays → twii_intraday → twii_daily → margin。`:1150-1220`
- **B-FLOW-6** 沿用舊資料的粒度彙總：macro＝**整鍵**（`old.macro or []`）；events＝**按 type**（dividend 一組、meeting 一組、conference＋earnings 一組，三組獨立）；top100＝整鍵＋保留 `top100_date`；punish＝**按 market**；quotes＝**逐檔按 name**（SOFR 另計）；holidays＝整鍵（舊值非 None 才沿用，否則省略）；twii_intraday／twii_daily／margin＝**各鍵獨立**（含其日期，全新安裝又失敗則省略）。已知取捨：單一來源解析中途壞一筆（`except Exception` 層級）會整類改用舊快照，不保留部分結果——**例外**見 B-DIV-6。
- **B-FLOW-7** `any_fresh` 的組成（`fetched` 刷新條件）：macro 非 None、除權息非 None、股東會非 None、top100 **非快取** 新抓成功、conference 最終非 None（含備援）、SOFR 本身抓到（不含沿用）、holidays 非 None。**不計**：punish、Yahoo quotes、三個動態桌布鍵、top100 快取命中。全部失敗＝`fetched` 沿用舊值（`old.fetched or old.updated or now_str`）。`:1146-1208`、`:1245`
- **B-FLOW-8** 前端據 `fetched` 顯示「資料更新」與「已 N 天未更新」（>3 天），據 `updated` 判斷是否重繪（`finance-calendar.html:827` `d.updated === lastGoodData.updated` 即跳過），故 `updated` 每輪必須變（分鐘解析度；同一分鐘內重跑相同）。
- **B-FLOW-9** `errors` 字串清單（Rust 版建議逐字保留以利日誌比對；前端僅計數）：
  - `啟動時網路等待逾時，改用既有資料兜底`
  - `總經來源失敗（本週檔）：{e}`
  - `除權息來源失敗：{e}`
  - `股東會來源失敗：{e}`
  - `市值前百大來源失敗：{e}`
  - `法說會：無市值前百大名單可篩，本輪改用備援來源`
  - `法說會來源（MOPS）失敗：{e}`
  - `法說會備援來源（重大訊息）失敗：{e}`
  - `處置股來源失敗（上市）：{e}`／`處置股來源失敗（上櫃）：{e}`
  - `行情來源失敗（{name}／{symbol}）：{e}`
  - `行情來源失敗（SOFR 3M／NY Fed）：{e}`
  - `休市日曆來源失敗：{e}`
  `wallpaper_errors`：`加權盤中走勢來源失敗：{e}`、`加權日K來源失敗：{e}`、`加權日K來源回傳較舊的資料（…），沿用上次資料`、`券資比／融資維持率來源失敗：{e}`、`{label}來源發生非預期錯誤：{類名}: {e}`。
- **B-FLOW-10** `errors` 是**每輪重算**的清單（不累積舊輪）；「失敗但沿用」仍會留下錯誤字串（前端據此顯示「⚠ N 個來源異常」）。AGENTS.md 所說「errors 照實累積」指同一輪內累積。
- **B-FLOW-11** `window`、`macro_meta`、`sources` 為常值／當輪計算，與來源成敗無關。

### 6.2 輸出目錄、原子寫檔、結束碼

- **B-FLOW-12** 輸出目錄清單＝`[腳本所在資料夾] + [Path(a) for a in sys.argv[1:]] + lively_wallpaper_dirs()`。Rust 版對應：宿主 `data_dir`（預設 `%LOCALAPPDATA%\tw.fintools.fc-host\data`，見 `host/src/data.rs:38-41`、AGENTS.md「資料流」）＝主目錄；舊輸出也應從此處讀（B-FLOW-3）。`:1281-1283`
- **B-FLOW-13** 每個目錄依序：`mkdir(parents=True, exist_ok=True)`；寫 `tw_events.json.tmp`、`tw_events.js.tmp`；`os.replace(json_tmp, json)` 再 `os.replace(js_tmp, js)`（**先 json 後 js**，兩次替換非整體原子）；成功 log `已輸出 → {d}`；任何例外 log `輸出到 {d} 失敗：{e}` 並**繼續下一目錄**（失敗時 `.tmp` 殘檔不清理）。`:1287-1301`
- **B-FLOW-14** 結束碼：`return 0 if ok else 1`——**只有「一個目錄都寫不成」才回 1**；即使所有來源全失敗、只寫出沿用的舊資料也回 0。程式碼層未捕捉的例外（見 B-FF-10／K-14）會使 Python 以 traceback 退出（碼 1）且不寫檔。`:1315`、`:1318-1319`
- **B-FLOW-15** **Lively 專屬（Rust 版可丟）** `lively_wallpaper_dirs()`：`%LOCALAPPDATA%\Packages\12030rocksdanister.LivelyWallpaper_*\LocalCache\Local\Lively Wallpaper\Library\**\finance-calendar.html` 的父目錄集合（排序去重）；`LOCALAPPDATA` 未設或 `OSError`→`[]`；找不到靜默略過。原因：Lively 整包複製桌布且其 WebView2 擋絕對 `file://` 跨資料夾讀取。`:1100-1113`
- **B-FLOW-16** **Lively 專屬（Rust 版可丟）**：`sys.argv[1:]` 額外輸出目錄（「萬用後路」）；`tw_events.js`（B-OUT-3）；排程器 `TW財經桌布資料更新`（`scripts/setup.ps1:157-170`，實況見第 9 節 D-5）。
- **B-FLOW-17** 非 Lively 專屬、Rust 版必須保留：舊輸出讀取＋全部沿用邏輯、原子寫檔（先寫 `.tmp` 再 rename，防宿主 30 秒輪詢讀到半份；宿主以 mtime＋size 判斷變更並讀檔，`host/src/data.rs:9-10`）、寫檔後內容必須是合法 JSON。
- **B-FLOW-18** 日誌輸出（stdout，UTF-8）：`[網路] …`、`抓取…（start ~ end）…`、`[總經] 下週檔尚未發布（…）`／`本輪失敗，沿用上次資料`、`[除權息] 即時 API 失敗（…），改用 openapi 備援`／`本輪失敗，沿用上次資料`、`[股東會] 本輪失敗，沿用上次資料`、`[前百大] 本輪失敗，沿用上次名單`、`[法說會] MOPS 來源失敗，改用重大訊息備援`／`本輪失敗，沿用上次資料`、`[處置股] 上市沿用上次資料（N 筆）`、`[行情] …，跳過`／`{name} 沿用上次資料`／`SOFR 3M 沿用上次資料`、`[休市日曆] 來源失敗，略過：…`／`本輪失敗，沿用上次資料`、`[盤中走勢]`／`[日K]`／`[券資比]` 各種沿用、`已輸出 → {d}`、結尾一行摘要：`完成：總經 N、除權息 N、股東會 N、財報 N、法說會 N 筆、處置 N 筆（上市 N／上櫃 N）、行情 N/13、休市日曆 N 筆（或「抓取失敗」）[；警告 N 項：…][；桌布資料警告 N 項：…]`（`quote_total=len(QUOTES)+1`）。`:1303-1314`。`setup.ps1` 的實跑驗證會 grep「警告／失敗」字樣上色（`scripts/setup.ps1:190+`），Rust 版若取代此腳本無須相容。

## 7. 常數表（C）

- **C-1** 可調參數 `:89-95`：`WINDOW_DAYS=14`、`INCLUDE_PAST_DAYS=0`、`MACRO_CURRENCIES=("USD","EUR","JPY")`、`MACRO_IMPACTS=("High","Medium")`、`TOP_N=100`、`TIMEOUT=30`。
- **C-2** `QUOTES`（`:103-116`，順序即顯示順序；Yahoo symbol／顯示名／kind）：
  1. `USDTWD=X`／`USD/TWD`／fx
  2. `^TNX`／`US 10Y`／yield
  3. `CL=F`／`WTI 原油`／cmdty
  4. `^TWII`／`加權指數`／index
  5. `2330.TW`／`台積電`／stock
  6. `^N225`／`日經225`／index
  7. `^KS11`／`KOSPI`／index
  8. `^STOXX50E`／`歐股50`／index
  9. `^DJI`／`道瓊`／index
  10. `^GSPC`／`S&P 500`／index
  11. `^IXIC`／`NASDAQ`／index
  12. `^SOX`／`費半`／index
  13.（不在 `QUOTES`，`fetch_sofr` 結果）`SOFR 3M`／yield，永遠 append 最後。合計 13 檔。
  注意：`:99-100` 註解寫「十一檔」「合計 12 檔」是過期文字，實際 12＋1。`QUOTES` 長度 12（已用 Python 驗證）。
- **C-3** 動態桌布常數 `:151-155`：`MARKET_OPEN=09:00`、`MARKET_CLOSE=13:30`、`INTRADAY_STEP=300`（秒）、`DAILY_MIN=40`、`MARGIN_SUM_TOLERANCE=0.01`。
- **C-4** 端點常數 `:122-149`：`URL_FF_THIS/NEXT`、`URL_DIV_RWD`、`URL_DIV_OAPI`、`URL_MEETING`、`URL_NEWS`、`URL_MOPS_CONF`、`URL_BASIC`、`URL_DAY_ALL`、`URL_PUNISH_TWSE`、`URL_PUNISH_TPEX`、`URL_QUOTE`、`URL_HOLIDAY`、`URL_SOFR`、`URL_SOFR_FALLBACK`、`URL_TWII_INTRADAY`、`URL_TWII_DAILY`、`URL_MARGN`（完整網址見各節）。
- **C-5** `UA` header `:119`；`TPE` `:120`；`TRADING_DAY_NOTICE` `:138`；`FETCH_ERRORS` `:162-163`；`CCY` `:165-166`。
- **C-6** 總經中譯字典 `T`：`:169-256`，94 條 `英文標題→中文`（Fed／ECB／BOJ 決議與談話、美國 NFP／CPI／PPI／零售／ISM／PMI、德法西歐元區指標、日本短觀／CPI／家庭支出等），**資料結構＝精確字串 map**；移植時直接整份抄成 Rust `phf`／`HashMap`，並以 `python -c "import update_tw_events as m; print(len(m.T))"` 對帳條數（94）。後綴取代規則見 B-FF-9。
- **C-7** Regex 常數：`RE_TR/RE_TD/RE_TAG/RE_CODE`（`:458-461`）、`ZH_NUM/RE_Q_ZH/RE_Q_EN`（`:463-465`）、`RE_STOCK_CODE/RE_ETF_CODE`（`:592-593`）、`roc_to_date` 兩條（`:332/:334`）。原文已逐條抄在各節。

## 8. 消費端：讀 `tw_events.json` 的程式與鍵（U）

> 結論：Rust 版必須保證下列所有鍵與子鍵的**名稱、型別、語意**不變。要害限制見每條「依賴」。

- **U-1** `host/src/data.rs`
  - `:181-185` `validate_tw_events`：`macro`、`events`、`punish`、`quotes` 四個陣列長度總和 `>0` 才算有效快照；全空或四鍵缺／非陣列→**整份拒收、保留舊快照**。`holidays` 不計入（`:166-180` 說明移植自 Lively `isEmptyPayload`）。**後果**：Rust 資料層寫出「四陣列全空」的檔，宿主會視為無效而不更新；以及純 `twii_*`／`margin` 更新但四陣列全空的檔也不會被宿主採納。
  - `:417-418` 頂層字串 `fetched`、`updated`→`SnapshotMeta`。
  - `:54-61` `WALLPAPER_DATA_KEYS`＝`twii_intraday`、`twii_daily`、`margin`、`holidays`、`events`、`wallpaper_errors`：存在才帶、**值原樣帶過去（含 `null`）**；`:65-75` `extract_wallpaper_data`。
  - `:38-41` 通道名 `tw-events`、檔名 `tw_events.json`（註解：「凍結不可改」）。
- **U-2** `host/src/wallpaper_coordinator.rs:249-260` `data_dates`：`twii_intraday.date`、`twii_daily` 由後往前**最後一筆有效的 `date`**、`margin.date`；字串須能被 `CivilDate::parse_iso`（ISO `YYYY-MM-DD`）解析；缺漏→`None`。`host/src/wallpaper.rs:211-213,868-880`：`twii_intraday` 缺→脊線／等高線等資料；`twii_daily` 缺→天際線等資料；`margin.date` 是撕日曆重畫觸發之一。
- **U-3** `host/ui/widgets/macro.js:96-160`：`macro[]` 的 `ts`（number 才用，否則退回 `date`/`time`/`dt`）、`date`、`time`、`dt`、`impact`（`high` 或 `medium`，用於 class 與判斷）、`country`（CSS class：`US`/`EU`/`JP`，`widget.css:314-334`）、`flag`、`title`、`forecast`、`previous`；頂層 `macro_meta.countries`／`macro_meta.importance`、`updated`。
- **U-4** `host/ui/widgets/dynamic.js:153-250`：`events` 必須是陣列；`events[]` 的 `type`（`earnings|dividend|meeting|conference`，依 `TYPE_META` 與 `dynTypes` 過濾，其餘 type 被濾掉不顯示）、`date`（ISO 字串比較）、`code`、`name`、`note`（前處理 `replace(/^除[權息]+\s*/,'')`，故除權息 note 必須以「除」開頭）；`holidays[]`（ISO 字串集合）；`punish[]` 的 `code`、`name`、`start`、`end`（ISO 字串比較）、`times`（number，`>=2` 顯示「第N次」）、`market`（`"上櫃"` 顯示「櫃・」）；頂層 `fetched`（退回 `updated`；以 `new Date(x.replace(' ','T'))` 當本機時間解析，故格式必須是 `YYYY-MM-DD HH:MM`）、`errors`（`.length`）。
- **U-5** `host/ui/widgets/quotes.js:120-142`：`quotes[]` 的 `name`、`kind`（`yield|fx|cmdty|index|stock`，其餘走 `String(price)`）、`price`（必須是 number，`.toFixed`）、`chg_abs`（yield 用）、`chg_pct`（其他用）。`prev`、`t` 未被讀取。
- **U-6** 動態桌布頁面（`host/ui/wallpapers/lib/`，只讀上列 `WALLPAPER_DATA_KEYS`）：
  - `intraday.mjs:30-41`：`twii_intraday` 必須是物件；`date` 為真實 ISO 日期；`points` 為陣列、非空；每點 `[number(有限), number(有限且>0)]`，壞點剔除。
  - `skyline-model.mjs:42-72`：`twii_daily` 為非空陣列；每列 `date`（真實日期）＋`open/high/low/close`（有限正數且 `high>=max(open,close,low)`、`low<=min(open,close)`），同日重複取後者；至少需足以算 MA（`MA_PERIOD`）。
  - `tearoff-model.mjs:142-165`：`events` 中 `code=="2330"` 且 `type∈{earnings,conference}` 且 `date==當天`；`margin.short_margin_ratio`、`margin.maintenance_ratio`（number）與 `margin.date`（字串）。
  - `tw-trading-days.mjs:57-66`：`holidays[]`（ISO 字串，`includes(date)`）。
  - `wallpaper.mjs:212`：`fetched`、`updated`；`_demo.html:66`：`quotes`、`twii_intraday.points`、`twii_daily`（示範用）。
  - `wallpaper_errors`：宿主轉傳，頁面端 repo 內**無讀取者**（僅 `core.mjs:29` 註解提及）。
- **U-7** 不讀 `tw_events.json`：`host/ui/widgets/clock.js`、`fixed.js`（純本地計算，`fixed.js:7`）、`custom*.js`（讀 `customN.json`）。
- **U-8** 凍結的 Lively 版 `finance-calendar.html`（唯讀）：`:459-510`（`macro`、`macro_meta`、`updated`，事件欄位同 U-3）、`:618-686`（`events`、`holidays`、`punish`、`fetched`/`updated`、`errors`）、`:768`（`quotes`）、`:795-837`（`isEmptyPayload`＝`macro+events+punish+quotes` 全空即拒收；`d.updated` 變才重繪；`window.TW_EVENTS`／`tw_events.json` 備援）。
- **U-9** 測試／工具端：`tests/compare_lively_dom.mjs`（以 `tests/fixtures/tw_events_sample.json` 產 `tw_events.js` 比對 DOM）；`host/tests/compare/serve.mjs:49-85`；`host/tests/wallpapers/*.test.mjs`（fixtures 內含各種 tw_events 形狀）；`host/tools/verify-*.ps1`（多數只複製 `host/ui/fixtures/tw-events.json` 到資料目錄當輸入，如 `verify-3.1.ps1:164`）。
- **U-10** 鍵／語意匯總（Rust 必保）：頂層 `updated`、`fetched`、`errors`、`macro`、`macro_meta`、`events`、`punish`、`quotes`、`holidays`、`twii_intraday`、`twii_daily`、`margin`、`wallpaper_errors`；**無人讀但仍為既有契約**：`window`、`counts`、`sources`、`top100`、`top100_date`（後兩者資料層自用，Rust 版要保留以維持「每日算一次」語意）。

## 9. 與 AGENTS.md「現況」不一致處（以程式為準）

- **D-1** AGENTS.md／任務描述稱腳本約 985 行；實際 1319 行（含動態桌布三鍵）。
- **D-2** `update_tw_events.py:99-100` 註解稱 QUOTES「十一檔」「合計 12 檔」已過期；實際 QUOTES 12 檔＋SOFR＝13（AGENTS.md「預設 13 檔」與 `:45` docstring「十三檔」才對）。
- **D-3** AGENTS.md 稱「單一來源解析中途壞一筆會整類改用舊快照（不保留部分結果）」：除權息主端點中途失敗時 `ev` 殘留部分列（B-DIV-6）；總經逐列 `continue`（壞一筆只略過該筆）。
- **D-4** AGENTS.md 稱 `top100` 為空視為來源失敗「交呼叫端走退回鏈」：實際是 `errors` 會**多一筆** `法說會：無市值前百大名單可篩…`，並走 B-NEWS 備援，備援成功則 `earnings` 型別事件整批消失（B-CONF-10）。
- **D-5** 排程：AGENTS.md 寫「每日 06:00 起每 6h＋登入後 2 分鐘」；`scripts/setup.ps1:157-175` 實際另有**每日 15:00** 與**睡眠／休眠喚醒（Power-Troubleshooter Event ID 1）立即觸發**；README.md:44 寫「每 6 小時＋每日 15:00＋每次登入」。Rust 版常駐宿主後這些觸發需在宿主內重新定義排程。
- **D-6** AGENTS.md 現況「韌性」稱「errors 照實累積」：每輪重算，非跨輪累積（B-FLOW-10）。
- **D-7** AGENTS.md 把動態桌布三鍵格式放在「現況」資料層一節（`AGENTS.md:26-29`，本 worktree 版），與本規格 B-ID／B-DK／B-MG 一致；無衝突。

## 10. 測試與 fixture 現況（T）

- **T-1** 唯一的資料層單元測試：`tests/test_wallpaper_data.py`（722 行、54 個測試，標準庫 `unittest`；執行 `uv run --no-project python -m unittest discover -s tests -v`）。注入 `now`，`mock.patch` 掉 `http_json`／`http_bytes`／`wait_for_network`／`lively_wallpaper_dirs`，**不發網路**。
- **T-2** 涵蓋範圍（按類）：`TwiiIntradayTest`（15）盤中走勢：收盤後取當日、盤中不覆蓋、午間不覆蓋、即時報價點剔除、13:30 點保留、null 剔除、序列過舊／缺口＞5 分鐘、週末取上一完整日、不倒退、用台北時鐘不用系統時區、失敗沿用／無舊值回 `None`、壞回應沿用。`TwiiDailyTest`（10）日 K：排序、當日收盤前排除、null 列與重複列、不足 40 根、失敗沿用、不倒退（記錯誤）、截斷回應。`MarginTest`（15）：券資比 2.53%（9/30）、個股加總＝彙總、`STOCK_DAY_ALL` 日期與空價略過、混日期拒收、`unpriced`／ETF 計入、手算小案例、不一致拒收、零融資拒收、同日價格才算且依日期請求、價格較新沿用、無舊值回 `None`、同日不重載大表、失敗／`stat≠OK`／MS-ALL 日期不一致。`MalformedOldTest`（3）：舊值型別不對當沒有。`MainIntegrationTest`（4）：截斷回應與 `OverflowError` 不使整輪停、非預期例外不使整輪停、新鍵不刷新 `fetched`、新鍵失敗進 `wallpaper_errors` 而非 `errors`。`HolidaysTest`（6）：交易日說明列排除、結算日保留、一般假日保留、2021–2026 逐列分類、Description 第一句規則。`OutputShapeTest`（1）：新鍵可 JSON 序列化。
- **T-3** **完全沒有測試**的行為（Rust 版需自行補，見 K 區）：`fetch_macro`、`zh_title`、`fetch_dividend`／`fmt_cash`、`fetch_meeting`、`fetch_top100`、`fetch_conference`／`mops_rows`／`quarter_of`、`fetch_conference_news`、`fetch_punish`／`parse_disposition_period`、`fetch_quotes`／`prev_close_from_chart`、`fetch_sofr`、`roc_to_date`、`wait_for_network`、`http_bytes` 的 SSL 退回、B-EVT 的窗口／去重／排序、`fetched` 的 `any_fresh` 組合（除新鍵外）、輸出原子寫檔。
- **T-4** Fixture：`tests/fixtures/`（說明 `README.md`，錄製日 2026-10-02 台北 10:46–14:36）：`yahoo_twii_5m_5d.json`、`yahoo_twii_1d_3mo.json`、`margn_ms_2026{0930,1001}.json`、`margn_all_2026{0930,1001}.json`、`stock_day_all_20261001.json`、`holiday_schedule_20261002.json`、`holiday_schedule_rwd_{2021..2025}_20261002.json`、`tw_events_sample.json`（完整輸出樣本，18 個頂層鍵，供 `compare_lively_dom.mjs`、宿主桌布 fixtures 使用；可當 Rust 輸出的**形狀對照基準**）。`README.md` 另記 Yahoo `interval`／`range`、`MI_MARGN`、`STOCK_DAY_ALL` 的查證結果。
- **T-5** 宿主端與資料層格式相關的測試：`host/src/data.rs`（`validate_tw_events` 與快照／衍生通道，約 `:791-1035`、`:1377-1590`）、`host/src/self_test_ipc.rs:106-129`、`host/src/widgets.rs:3847-3960`；`host/tests/wallpapers/*.test.mjs` 與 `fixtures/`（頁面端讀鍵的容錯／拒收行為）；`host/tests/dynamic-footer-age.test.mjs`（`fetched` 超過 3 天警示）。
- **T-6** 樣本：`host/ui/fixtures/tw-events.json`（宿主開發用 fixture，含 `top100_date: 2026-09-28`）。

## 11. 已知坑與移植風險（K）

- **K-1** `fetched`／`updated` 時區是**系統本機**，不是台北（B-DATE-3）；前端以本機解讀。Rust 用 `chrono::Local` 或等效，格式 `%Y-%m-%d %H:%M`，勿用 RFC3339。
- **K-2** **昨收不可用 `chartPreviousClose`**（B-QT-3／B-QT-4）：用 `gmtoffset` 做當地日序歸戶、取最後一個日序≠當前交易日的收盤，且「`rows` 非空但找不到」時回 `None` 不走倒數第二筆；`prev_close or chartPreviousClose` 的 falsy 語意（`0.0` 也退回）。
- **K-3** `top100` 快取語意：同日不發請求；失敗時**保留舊 `top100_date`**；`any_fresh` 對快取命中不算新（B-TOP-2／B-TOP-6）。排序同市值時代號降冪（B-TOP-5）。
- **K-4** 浮點捨入：Python `round(x, 2)`（盤中價 `round(float(c),2)`、日 K OHLC、`short_margin_ratio`、`maintenance_ratio`）依**浮點精確十進位展開**捨入（銀行家式於「恰好 .5」的二進位表示），與 Rust 常見的 `(x*100.0).round()/100.0` 在極少數邊界不同；`chg_pct`、`prev`、SOFR 不捨入。建議用與 Python 等效的作法並以 fixture（`tests/fixtures`）逐值對帳（見 `tests/test_wallpaper_data.py` 手算案例）。
- **K-5** Regex 的 Unicode 語意：`quarter_of` 的 `\b`、`\d`、`RE_CODE` 的 `\d` 在 Python str 模式皆 Unicode 感知；Rust `regex` 預設同為 Unicode，**不要用 `(?-u)` 或 ASCII 化**，否則「緊貼中文的 `2Q26`」這類案例會與 Python 分歧（Python 不命中）。`ZH` 規則另含全形／中文數字（`一二三四`）。
- **K-6** `fmt_cash` 的 `{v:g}`（B-DIV-4）：6 位有效數字、整數不帶小數點、科學記號門檻；Rust `{}` 不等價，需自寫 `%g`。
- **K-7** `MOPS` 抓兩個月、任一失敗整體失敗（不保留部分結果）；月份跨年進位與民國年不補零（B-CONF-2）；HTML 以 `utf-8`／`replace` 解碼；表格 regex 為非貪婪＋`re.S|re.I`。
- **K-8** 處置股：期間雙格式（全形～含斜線／半形~無斜線）；`(end,start)` 最大者勝；`times` 取保留筆；TPEx 空白樣板列要被代號過濾擋下；按 market 分段沿用舊資料（B-PUN-3～B-PUN-8）。
- **K-9** `holidays`：兩步判斷（Name 命中且 Description 第一句不含「無交易」才排除），且只看第一句；鍵省略的三種條件（B-HOL-2）。
- **K-10** 動態桌布三鍵「各自獨立沿用」「不計入 `fetched`」「錯誤進 `wallpaper_errors`」「`old` 型別不對當沒有」「日期不倒退」（intraday 靜默、daily 記錯誤）。
- **K-11** 宿主 `validate_tw_events` 拒收「四陣列全空」：Rust 資料層若在全來源失敗＋無舊資料時寫出全空檔，宿主會忽略它；與 Python 相同（Python 也會寫出全空檔）。若新增「僅更新桌布鍵」的流程，要確認此驗證不會吃掉。
- **K-12** 並非所有失敗都記進 `errors`：總經下週檔、除權息主端點、行情被沿用的舊值（仍記錯誤）、`wallpaper` 三鍵、TPEx 空白樣板列等各有不同記錄策略（B-FF-2、B-DIV-5、B-QT-7、B-GRD-2、B-PUN-4）。
- **K-13** `updated` 分鐘解析度＋前端「`updated` 不變就不重繪」：同一分鐘內第二輪不會觸發重繪（B-FLOW-8）。Rust 版若縮短輪詢，仍需保證新資料時 `updated` 改變（可考慮秒級，但前端以 `replace(' ','T')` 解析，須兼容）。
- **K-14** 未被接住的例外：`fetch_macro` 列轉換中的非字串 `forecast`／`previous` 會讓整個 `main()` 崩潰、不寫檔（B-FF-10）；Rust 版建議以「單列失敗略過」處理，這屬於刻意的行為改良，需在驗收時標明。
- **K-15** 結束碼語意（B-FLOW-14）：僅「所有輸出目錄都寫不成」回 1；全來源失敗仍回 0。排程／宿主若以結束碼判斷成敗要留意。
- **K-16** SSL 退回對所有主機放寬驗證（B-HTTP-4）：Rust 版是否保留需明確決策（安全面）；若不保留，須實測 `*.twse.com.tw` 在 rustls／native-tls 下能否通過驗證，否則全 twse 來源失敗而沿用舊資料。
- **K-17** `STOCK_DAY_ALL` 每輪下載兩次（top100 一次、margin 一次，且 top100 同日快取命中時只剩 margin 那次）；兩者都有「全表同日」與 `ClosingPrice` 空字串／`≤0` 不收的規則，但**top100 不檢查日期單一性**（只有 margin 版 `parse_day_all_prices` 檢查）。
- **K-18** 事件排序依字串升冪，`type` 字母序 `conference<dividend<earnings<meeting`；前端會依自己的 `dynTypes` 重排，但 JSON 輸出順序仍應維持（diff 與測試基準）。
- **K-19** 未列入條款的細節：JSON 鍵序、縮排寬度、CRLF 皆不影響消費端（B-OUT-2／B-OUT-6）。
