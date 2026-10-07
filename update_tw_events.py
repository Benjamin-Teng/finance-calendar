#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
update_tw_events.py — 財經日曆資料更新
輸出 tw_events.json 與 tw_events.js，供 finance-calendar.html 讀取渲染。

抓兩類資料（全部免金鑰、只用 Python 標準庫，免安裝任何套件）：

【總經日曆】Forex Factory 週曆 JSON（本週＋下週，含預測值/前值/重要性）
    https://nfs.faireconomy.media/ff_calendar_thisweek.json
    https://nfs.faireconomy.media/ff_calendar_nextweek.json
    篩選幣別 MACRO_CURRENCIES（美歐日）＋重要性 MACRO_IMPACTS（中高），
    時間轉台北時區，常見指標名稱翻成中文。
    ※「下週」檔通常接近週末才發布，平日抓不到屬正常（會自動略過）。
    ※ 原本用 TradingView widget，但 Wallpaper Engine 的 CEF 跑不動（死圖），
      改為腳本抓資料、HTML 自繪。

【台股動態事件】
  除權息   TWSE 除權除息預告表（即時）
           https://www.twse.com.tw/rwd/zh/exRight/TWT48U?response=json
           備援：https://openapi.twse.com.tw/v1/exchangeReport/TWT48U_ALL
  股東會   TWSE OpenAPI https://openapi.twse.com.tw/v1/opendata/t187ap41_L
  財報／法說會
           MOPS 法人說明會一覽表（POST，含未來月份場次）
           https://mopsov.twse.com.tw/mops/web/ajax_t100sb02_1
           只收市值前百大（TOP_N）；擇要訊息驗得出季別＝財報場次（type=earnings、
           note「Q2 財報」），驗不出＝一般法說會（type=conference）。
           日期欄有單日「115/07/16」與區間「115/06/30 至 115/07/03」兩種格式，取起日。
           上櫃（TYPEK=otc）不抓——實測前百大無一家上櫃。
           備援：TWSE OpenAPI 重大訊息（篩「第12款＝召開法人說明會」）
           https://openapi.twse.com.tw/v1/opendata/t187ap04_L
           ※ 備援只有「當日」公告、抓不到未來場次，故僅在 MOPS 失敗時使用。
  前百大   市值＝已發行普通股數 × 收盤價，取前 TOP_N 名
           https://openapi.twse.com.tw/v1/opendata/t187ap03_L（股數，1.3 MB）
           https://openapi.twse.com.tw/v1/exchangeReport/STOCK_DAY_ALL（收盤價）
           排名變動極慢，每日只算一次（快取於輸出的 top100／top100_date）。
  處置股   上市 TWSE OpenAPI（固定近期窗口，含處置中）
           https://openapi.twse.com.tw/v1/announcement/punish
           上櫃 TPEx OpenAPI（固定 snapshot，不支援日期參數）
           https://www.tpex.org.tw/openapi/v1/tpex_disposal_information
           只收股票／ETF，濾掉權證、可轉債等衍生商品代碼；二度處置時來源新舊公告
           並存，同 code 只保留期間最新（end 最大）那筆
  （FinMind 需逐檔查詢無法一次撈全市場，故以 TWSE 為主。）

【行情條】十三檔標的即時報價，供 HTML 底部行情條（單檔失敗只印警告後跳過，不影響其他檔）
    Yahoo Finance chart API（USD/TWD、美債殖利率、原油、加權指數、台積電、
      日經225、KOSPI、歐股50、道瓊、S&P500、NASDAQ、費半 共十二檔；symbol／顯示名／kind 見頂部 QUOTES）
           https://query1.finance.yahoo.com/v8/finance/chart/{symbol}?interval=1d&range=5d
    SOFR 3M（NY Fed 官方 API，免金鑰；fetch_sofr() 抓完 append 到清單最後）
           https://markets.newyorkfed.org/api/rates/secured/sofrai/last/2.json

【休市日曆】TWSE 公告休市日（僅當年度），供台股動態事件判斷「下一個交易日」
    https://openapi.twse.com.tw/v1/holidaySchedule/holidaySchedule
    跨年日期由前端（finance-calendar.html）的週末規則兜底，非致命失敗。

【動態桌布資料】三個新鍵 twii_intraday／twii_daily／margin（格式見 AGENTS.md「現況」資料層一節）
    加權指數盤中走勢（最後一個完整交易日）、日 K（≥40 根）：Yahoo v8 chart ^TWII
           https://query1.finance.yahoo.com/v8/finance/chart/%5ETWII?interval=5m&range=5d
           https://query1.finance.yahoo.com/v8/finance/chart/%5ETWII?interval=1d&range=3mo
    上市券資比、融資維持率：TWSE rwd MI_MARGN（MS 彙總／ALL 個股）＋ STOCK_DAY_ALL 收盤價
           https://www.twse.com.tw/rwd/zh/marginTrading/MI_MARGN?selectType=MS&response=json
    各鍵獨立失敗沿用上次輸出；參數與樣本的查證見 tests/fixtures/README.md。

用法：
  python update_tw_events.py
      → 輸出到本腳本所在資料夾（即 Wallpaper Engine 專案資料夾）
  python update_tw_events.py "D:\\其他資料夾"
      → 額外同步輸出到指定資料夾（可多個）
"""

import html
import http.client
import io
import json
import os
import re
import socket
import ssl
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from datetime import date, datetime, timedelta, timezone
from datetime import time as dtime
from itertools import pairwise
from pathlib import Path

# ─── 可調參數 ────────────────────────────────────────────────
WINDOW_DAYS = 14                       # 台股動態事件：今天 ～ 今天+14 天
INCLUDE_PAST_DAYS = 0                  # 想保留剛發生過的 N 天可調大
MACRO_CURRENCIES = ("USD", "EUR", "JPY")   # 總經：美元、歐元區、日圓
MACRO_IMPACTS = ("High", "Medium")         # 總經重要性：高、中
TOP_N = 100                            # 財報只收市值前 N 大（法說會場次太多，前百大才是關注標的）
TIMEOUT = 30

# 行情條：(yahoo_symbol, 顯示名, kind)；kind ∈ fx/yield/cmdty/index/stock，
# 決定 finance-calendar.html 的顯示格式（小數位／千分位／%）。
# 本清單十一檔皆為 Yahoo 標的；另有 SOFR 3M（NY Fed，非 Yahoo）由 fetch_sofr()
# 抓取後於 main() append 到 quotes 清單最後一筆，顯示順序＝本清單順序→SOFR 3M，合計 12 檔。
# ※ 櫃買指數 Yahoo symbol（^TWOII）已停更（樣本 regularMarketTime 停在 2024-10），
#   要加櫃買改走 TPEx 官方 https://www.tpex.org.tw/openapi/v1/tpex_index（本次不實作）。
QUOTES = [
    ("USDTWD=X", "USD/TWD", "fx"),
    ("^TNX", "US 10Y", "yield"),
    ("CL=F", "WTI 原油", "cmdty"),
    ("^TWII", "加權指數", "index"),
    ("2330.TW", "台積電", "stock"),
    ("^N225", "日經225", "index"),
    ("^KS11", "KOSPI", "index"),
    ("^STOXX50E", "歐股50", "index"),
    ("^DJI", "道瓊", "index"),
    ("^GSPC", "S&P 500", "index"),
    ("^IXIC", "NASDAQ", "index"),
    ("^SOX", "費半", "index"),
]
# ────────────────────────────────────────────────────────────

UA = {"User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) finance-calendar/1.0"}
TPE = timezone(timedelta(hours=8))     # 台北（無夏令時間，固定 +8）

URL_FF_THIS = "https://nfs.faireconomy.media/ff_calendar_thisweek.json"
URL_FF_NEXT = "https://nfs.faireconomy.media/ff_calendar_nextweek.json"
URL_DIV_RWD  = "https://www.twse.com.tw/rwd/zh/exRight/TWT48U?response=json"
URL_DIV_OAPI = "https://openapi.twse.com.tw/v1/exchangeReport/TWT48U_ALL"
URL_MEETING  = "https://openapi.twse.com.tw/v1/opendata/t187ap41_L"
URL_NEWS     = "https://openapi.twse.com.tw/v1/opendata/t187ap04_L"
# 法說會／財報：MOPS 法人說明會一覽表。openapi 的 t187ap04_L 只有「當日」重大訊息，
# 未來場次抓不到；這張表含未來月份，擇要訊息即財報公布聲明。舊站 mopsov 才吃這條 ajax。
URL_MOPS_CONF = "https://mopsov.twse.com.tw/mops/web/ajax_t100sb02_1"
URL_BASIC    = "https://openapi.twse.com.tw/v1/opendata/t187ap03_L"      # 含已發行普通股數
URL_DAY_ALL  = "https://openapi.twse.com.tw/v1/exchangeReport/STOCK_DAY_ALL"  # 含收盤價
URL_PUNISH_TWSE = "https://openapi.twse.com.tw/v1/announcement/punish"
URL_PUNISH_TPEX = "https://www.tpex.org.tw/openapi/v1/tpex_disposal_information"
URL_QUOTE    = "https://query1.finance.yahoo.com/v8/finance/chart/{symbol}?interval=1d&range=5d"
URL_HOLIDAY  = "https://openapi.twse.com.tw/v1/holidaySchedule/holidaySchedule"
# holidaySchedule 裡「交易日」說明列的 Name 特徵（開始交易日／最後交易日）；還要再排除 Description 第一句寫「無交易」者
TRADING_DAY_NOTICE = re.compile(r"(開始|最後)交易")
URL_SOFR     = "https://markets.newyorkfed.org/api/rates/secured/sofrai/last/2.json"
URL_SOFR_FALLBACK = "https://markets.newyorkfed.org/api/rates/secured/sofrai/last/5.json"
# 動態桌布用的三個新鍵（twii_intraday／twii_daily／margin）。
# interval／range 的查證依據與實測見 tests/fixtures/README.md：
#   5m 資料 Yahoo 只提供最近 60 天（range=3mo 會被拒）、meta.validRanges 含 5d；range=5d＝最近 5 個
#   「交易日」（不是日曆日；實測跨過 9/25 中秋、9/28 補假仍給滿 5 個交易日），連假也撈得到上一個完整日。
URL_TWII_INTRADAY = "https://query1.finance.yahoo.com/v8/finance/chart/%5ETWII?interval=5m&range=5d"
URL_TWII_DAILY = "https://query1.finance.yahoo.com/v8/finance/chart/%5ETWII?interval=1d&range=3mo"
# MI_MARGN：selectType=MS 取彙總（融資／融券張數、融資金額仟元）、ALL 取個股明細；
# 不帶 date 回「最新已公布」那天（回應的 date 欄為 YYYYMMDD），帶 date 可指定日（查無資料 stat≠OK）
URL_MARGN = "https://www.twse.com.tw/rwd/zh/marginTrading/MI_MARGN?selectType={kind}&response=json"

MARKET_OPEN = dtime(9, 0)              # 台股盤中 09:00–13:30（含收盤試撮）
MARKET_CLOSE = dtime(13, 30)
INTRADAY_STEP = 300                    # 盤中走勢相鄰間隔上限（秒）＝5 分鐘 K
DAILY_MIN = 40                         # 日 K 至少 40 根，讓最近 20 根都算得出 20MA
MARGIN_SUM_TOLERANCE = 0.01            # 個股明細加總與彙總張數的容許相對誤差
# 抓取／解析可預期的失敗：網路與 HTTP（OSError 含 URLError／HTTPError／逾時／SSL）、回應本體傳到一半斷線或
# 狀態列壞掉（http.client.HTTPException：IncompleteRead、BadStatusLine、LineTooLong——它們不是 OSError）、
# JSON 壞掉或編碼錯（ValueError，含 json.JSONDecodeError、UnicodeDecodeError）、數值或時間戳離譜
# （ArithmeticError，含 datetime.fromtimestamp 的 OverflowError）、回應欄位缺漏或形狀不對
# （LookupError／TypeError／AttributeError）。task 6.4（審查 R3-M）補上 HTTPException 與 ArithmeticError；
# 白名單以外的例外由 main() 的 guarded_wallpaper_fetch 兜底，新鍵的失敗絕不讓整輪不寫檔。
FETCH_ERRORS = (OSError, ValueError, LookupError, TypeError, AttributeError, ArithmeticError,
                http.client.HTTPException)

CCY = {"USD": ("US", "美"), "EUR": ("EU", "歐"), "JPY": ("JP", "日"),
       "CNY": ("CN", "中"), "GBP": ("GB", "英")}

# 常見指標中譯（沒對到的套字尾規則，再沒有就保留英文）
T = {
    "Non-Farm Employment Change": "非農就業人數",
    "ADP Non-Farm Employment Change": "ADP 非農就業",
    "Unemployment Rate": "失業率",
    "Average Hourly Earnings m/m": "平均時薪 (月增)",
    "Unemployment Claims": "初請失業金人數",
    "CPI m/m": "CPI (月增)", "CPI y/y": "CPI (年增)",
    "Core CPI m/m": "核心CPI (月增)",
    "PPI m/m": "PPI (月增)", "PPI y/y": "PPI (年增)",
    "Core PPI m/m": "核心PPI (月增)",
    "Core PCE Price Index m/m": "核心PCE物價 (月增)",
    "Retail Sales m/m": "零售銷售 (月增)", "Retail Sales y/y": "零售銷售 (年增)",
    "Core Retail Sales m/m": "核心零售銷售 (月增)",
    "ISM Manufacturing PMI": "ISM 製造業PMI",
    "ISM Services PMI": "ISM 非製造業PMI",
    "Flash Manufacturing PMI": "製造業PMI初值",
    "Flash Services PMI": "服務業PMI初值",
    "Final Manufacturing PMI": "製造業PMI終值",
    "Final Services PMI": "服務業PMI終值",
    "Prelim UoM Consumer Sentiment": "密大消費者信心初值",
    "Revised UoM Consumer Sentiment": "密大消費者信心終值",
    "CB Consumer Confidence": "諮商會消費者信心",
    "Federal Funds Rate": "Fed 利率決議",
    "FOMC Statement": "FOMC 聲明", "FOMC Press Conference": "FOMC 記者會",
    "FOMC Meeting Minutes": "FOMC 會議紀要",
    "FOMC Economic Projections": "FOMC 經濟預測",
    "Fed Chair Powell Speaks": "Fed 主席鮑爾談話",
    "Fed Chair Powell Testifies": "Fed 主席鮑爾聽證",
    "Fed Monetary Policy Report": "Fed 貨幣政策報告",
    "Empire State Manufacturing Index": "紐約州製造業指數",
    "Philly Fed Manufacturing Index": "費城聯儲製造業指數",
    "Industrial Production m/m": "工業生產 (月增)",
    "Housing Starts": "新屋開工", "Building Permits": "營建許可",
    "Existing Home Sales": "成屋銷售", "New Home Sales": "新屋銷售",
    "Pending Home Sales m/m": "成屋簽約銷售 (月增)",
    "Durable Goods Orders m/m": "耐久財訂單 (月增)",
    "Core Durable Goods Orders m/m": "核心耐久財訂單 (月增)",
    "Advance GDP q/q": "GDP 季增初值", "Prelim GDP q/q": "GDP 季增修正值",
    "Final GDP q/q": "GDP 季增終值",
    "JOLTS Job Openings": "JOLTS 職位空缺",
    "Trade Balance": "貿易帳",
    "Crude Oil Inventories": "EIA 原油庫存",
    "Natural Gas Storage": "EIA 天然氣庫存",
    "Personal Income m/m": "個人所得 (月增)",
    "Personal Spending m/m": "個人支出 (月增)",
    "Factory Orders m/m": "工廠訂單 (月增)",
    "Consumer Credit m/m": "消費信貸",
    "Main Refinancing Rate": "歐洲央行利率決議",
    "Monetary Policy Statement": "貨幣政策聲明",
    "ECB Press Conference": "歐洲央行記者會",
    "ECB Monetary Policy Meeting Accounts": "歐洲央行會議紀要",
    "ECB President Lagarde Speaks": "歐洲央行總裁拉加德談話",
    "German ZEW Economic Sentiment": "德國ZEW景氣指數",
    "ZEW Economic Sentiment": "歐元區ZEW景氣指數",
    "German ifo Business Climate": "德國ifo商業景氣",
    "German Prelim CPI m/m": "德國CPI初值 (月增)",
    "German Final CPI m/m": "德國CPI終值 (月增)",
    "French Final CPI m/m": "法國CPI終值 (月增)",
    "French Flash CPI m/m": "法國CPI初值 (月增)",
    "Spanish Flash CPI y/y": "西班牙CPI初值 (年增)",
    "CPI Flash Estimate y/y": "歐元區CPI初值 (年增)",
    "Core CPI Flash Estimate y/y": "歐元區核心CPI初值 (年增)",
    "Final CPI y/y": "CPI終值 (年增)", "Final Core CPI y/y": "核心CPI終值 (年增)",
    "German Factory Orders m/m": "德國工廠訂單 (月增)",
    "German Industrial Production m/m": "德國工業生產 (月增)",
    "German Trade Balance": "德國貿易帳",
    "Sentix Investor Confidence": "Sentix投資人信心",
    "German Flash Manufacturing PMI": "德國製造業PMI初值",
    "German Flash Services PMI": "德國服務業PMI初值",
    "BOJ Policy Rate": "日銀利率決議",
    "BOJ Outlook Report": "日銀展望報告",
    "BOJ Press Conference": "日銀記者會",
    "Monetary Policy Meeting Minutes": "日銀會議紀要",
    "BOJ Gov Ueda Speaks": "日銀總裁植田談話",
    "Tankan Manufacturing Index": "短觀製造業指數",
    "Tankan Non-Manufacturing Index": "短觀非製造業指數",
    "National Core CPI y/y": "全國核心CPI (年增)",
    "Tokyo Core CPI y/y": "東京核心CPI (年增)",
    "Household Spending y/y": "家庭支出 (年增)",
    "Average Cash Earnings y/y": "平均現金收入 (年增)",
    "Bank Lending y/y": "銀行放款 (年增)",
    "Current Account": "經常帳",
    "Economy Watchers Sentiment": "景氣觀察指數",
    "M2 Money Stock y/y": "M2貨幣供給 (年增)",
    "Prelim Machine Tool Orders y/y": "工具機訂單初值 (年增)",
    "Leading Indicators": "領先指標",
    "Bank Holiday": "休市",
}


def zh_title(t: str) -> str:
    if t in T:
        return T[t]
    s = t
    for a, b in ((" m/m", " (月增)"), (" y/y", " (年增)"), (" q/q", " (季增)"),
                 (" Speaks", " 談話"), (" Testifies", " 聽證")):
        s = s.replace(a, b)
    return s


def log(msg: str) -> None:
    try:
        print(msg)
    except UnicodeEncodeError:
        print(msg.encode("utf-8", "replace").decode("utf-8", "replace"))


def http_bytes(url: str, form: dict | None = None) -> bytes:
    """取原始 bytes；form 非 None 時以 POST 送 urlencoded 表單。"""
    headers = dict(UA)
    data = None
    if form is not None:
        data = urllib.parse.urlencode(form).encode()
        headers["Content-Type"] = "application/x-www-form-urlencoded"
    req = urllib.request.Request(url, data=data, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
            return r.read()
    except (ssl.SSLError, urllib.error.URLError) as e:
        # urlopen 會把握手層的 SSL 憑證錯誤包成 URLError(reason=SSLError)，故兩者都要接——
        # 否則較新的 OpenSSL（如 Python 3.13）對 twse 憑證鏈檢查較嚴時，下面的退回會被跳過。
        # 僅在確認是 SSL 問題時才退回「不驗證」重試；純網路錯誤／HTTPError 照原樣往上丟。
        reason = getattr(e, "reason", None)
        if not isinstance(e, ssl.SSLError) and not isinstance(reason, ssl.SSLError):
            raise
        ctx = ssl.create_default_context()
        ctx.check_hostname = False
        ctx.verify_mode = ssl.CERT_NONE
        with urllib.request.urlopen(req, timeout=TIMEOUT, context=ctx) as r:
            return r.read()


def http_json(url: str):
    return json.loads(http_bytes(url).decode("utf-8"))


def wait_for_network(host: str = "www.twse.com.tw", port: int = 443,
                      timeout: int = 5, interval: int = 20, max_wait: int = 300) -> bool:
    """開機排程補跑時網路常常還沒就緒；用原始 socket 連線輪詢（刻意不吃
    http(s)_proxy 環境變數，只測底層網路本身），每 interval 秒重試，
    累計等滿 max_wait 秒仍失敗就放行（後續交給 merge fallback 兜底），
    不讓腳本卡死在等待迴圈。"""
    waited = 0
    while True:
        try:
            with socket.create_connection((host, port), timeout=timeout):
                if waited:
                    log(f"[網路] 已就緒（等待 {waited} 秒）")
                return True
        except OSError as e:
            if waited >= max_wait:
                log(f"[網路] 等待逾時（{max_wait} 秒）仍未就緒，改用既有資料兜底：{e}")
                return False
            log(f"[網路] 尚未就緒（{e}），{interval} 秒後重試…")
            time.sleep(interval)
            waited += interval


def roc_to_date(s):
    """民國日期 → date。支援 1150715 / 115/07/15 / 115年07月15日"""
    if not s:
        return None
    s = str(s).strip()
    m = re.match(r"^(\d{2,3})[年/.\-](\d{1,2})[月/.\-](\d{1,2})日?$", s)
    if not m:
        m = re.match(r"^(\d{3})(\d{2})(\d{2})$", s)
    if not m:
        return None
    y, mo, d = (int(g) for g in m.groups())
    try:
        return date(y + 1911, mo, d)
    except ValueError:
        return None


def fmt_cash(cash: str) -> str:
    try:
        v = float(str(cash).strip())
        return f" 現金{v:g}元" if v > 0 else ""
    except (ValueError, TypeError):
        return ""


# ─── 總經日曆（Forex Factory） ──────────────────────────────

def fetch_macro(errors: list) -> list | None:
    """回傳 None＝本週檔（必要來源）失敗，呼叫端應整鍵沿用舊資料；
    下週檔缺席屬正常（見下方註解），不影響回傳。"""
    rows = []
    this_week_failed = False
    for url in (URL_FF_THIS, URL_FF_NEXT):
        try:
            rows += http_json(url) or []
        except Exception as e:
            if url == URL_FF_NEXT:
                # 「下週」檔 Forex Factory 通常接近週末才發布，抓不到屬正常，
                # 不列入錯誤（排程每 6 小時會自動補上）
                log(f"[總經] 下週檔尚未發布（{e}），先只用本週")
            else:
                errors.append(f"總經來源失敗（本週檔）：{e}")
                this_week_failed = True
    if this_week_failed:
        return None
    ev, seen = [], set()
    for r in rows:
        ccy = r.get("country", "")
        imp = r.get("impact", "")
        if ccy not in MACRO_CURRENCIES or imp not in MACRO_IMPACTS:
            continue
        try:
            dt = datetime.fromisoformat(r["date"]).astimezone(TPE)
        except Exception:
            continue
        key = (r.get("title"), r.get("date"))
        if key in seen:
            continue
        seen.add(key)
        code, disp = CCY.get(ccy, (ccy, ccy[:1]))
        ev.append({
            "dt": dt.strftime("%Y-%m-%dT%H:%M"),
            "date": dt.strftime("%Y-%m-%d"),
            "time": dt.strftime("%H:%M"),
            "ts": int(dt.timestamp()),
            "country": code, "flag": disp,
            "impact": "high" if imp == "High" else "medium",
            "title": zh_title(r.get("title", "")),
            "title_en": r.get("title", ""),
            "forecast": (r.get("forecast") or "").strip(),
            "previous": (r.get("previous") or "").strip(),
        })
    ev.sort(key=lambda e: e["dt"])
    return ev


# ─── 台股三類事件 ───────────────────────────────────────────

def fetch_dividend(errors: list) -> list | None:
    """除權息：TWSE RWD（即時）→ 失敗改 openapi。回傳 None＝兩來源皆失敗"""
    ev = []
    try:
        j = http_json(URL_DIV_RWD)
        for r in j.get("data") or []:
            d = roc_to_date(r[0])
            if not d:
                continue
            ev.append({"date": d.isoformat(), "type": "dividend",
                       "code": str(r[1]).strip(), "name": str(r[2]).strip(),
                       "note": "除" + (str(r[3]).strip() or "權息") + fmt_cash(r[7])})
        return ev
    except Exception as e:
        log(f"[除權息] 即時 API 失敗（{e}），改用 openapi 備援")
    try:
        for r in http_json(URL_DIV_OAPI):
            d = roc_to_date(r.get("Date"))
            if not d:
                continue
            ev.append({"date": d.isoformat(), "type": "dividend",
                       "code": str(r.get("Code", "")).strip(),
                       "name": str(r.get("Name", "")).strip(),
                       "note": "除" + (r.get("Exdividend") or "權息")
                               + fmt_cash(r.get("CashDividend"))})
        return ev
    except Exception as e:
        errors.append(f"除權息來源失敗：{e}")
        return None


def fetch_meeting(errors: list) -> list | None:
    """股東會（常會＋臨時會）。回傳 None＝來源失敗"""
    ev = []
    try:
        for r in http_json(URL_MEETING):
            d = roc_to_date(r.get("開會日期"))
            if not d:
                continue
            kind = str(r.get("股東常(臨時)會", "")).strip()
            note = ("股東" + kind) if kind else "股東會"
            if str(r.get("是否改選董監", "")).strip() == "是":
                note += "・改選董監"
            ev.append({"date": d.isoformat(), "type": "meeting",
                       "code": str(r.get("公司代號", "")).strip(),
                       "name": str(r.get("公司名稱", "")).strip(),
                       "note": note})
    except Exception as e:
        errors.append(f"股東會來源失敗：{e}")
        return None
    return ev


RE_TR = re.compile(r"<tr[^>]*>(.*?)</tr>", re.S | re.I)
RE_TD = re.compile(r"<t[dh][^>]*>(.*?)</t[dh]>", re.S | re.I)
RE_TAG = re.compile(r"<[^>]+>")
RE_CODE = re.compile(r"^\d{4}$")
# 季別：中文「第2季／第二季」與英文「2Q26／Q2」
ZH_NUM = {"一": 1, "二": 2, "三": 3, "四": 4, "1": 1, "2": 2, "3": 3, "4": 4}
RE_Q_ZH = re.compile(r"第\s*([一二三四1-4])\s*季")
RE_Q_EN = re.compile(r"\b([1-4])Q\d{2}\b|\bQ([1-4])\b", re.I)


def mops_rows(text: str) -> list[list[str]]:
    """MOPS 回傳的 HTML 表格 → 每列的純文字欄位；只留代號欄是 4 位數字的資料列"""
    rows = []
    for tr in RE_TR.findall(text):
        cells = [html.unescape(RE_TAG.sub("", c)).strip() for c in RE_TD.findall(tr)]
        if cells and RE_CODE.match(cells[0]):
            rows.append(cells)
    return rows


def quarter_of(s: str) -> int | None:
    """從法說會擇要訊息判斷是第幾季的財報場次；判不出＝非財報場（如券商邀約座談）"""
    m = RE_Q_ZH.search(s)
    if m:
        return ZH_NUM[m.group(1)]
    m = RE_Q_EN.search(s)
    if m:
        return int(m.group(1) or m.group(2))
    return None


def fetch_top100(errors: list, old: dict) -> list | None:
    """市值前 N 大代號。市值排名變動極慢，每日只算一次——同日直接沿用快取，
    省下 t187ap03_L（1.3 MB）＋ STOCK_DAY_ALL（0.3 MB）的重複下載。
    回傳 None＝本輪失敗（由 main 沿用舊名單）"""
    today = datetime.now(TPE).date().isoformat()
    cached = old.get("top100") or []
    if cached and old.get("top100_date") == today:
        return cached
    try:
        price = {}
        for r in http_json(URL_DAY_ALL):
            v = str(r.get("ClosingPrice") or "").replace(",", "").strip()
            try:
                if float(v) > 0:
                    price[str(r.get("Code") or "").strip()] = float(v)
            except ValueError:
                continue
        caps = []
        for b in http_json(URL_BASIC):
            code = str(b.get("公司代號") or "").strip()
            shares = str(b.get("已發行普通股數或TDR原股發行股數") or "").strip()
            if code in price and shares.isdigit():
                caps.append((int(shares) * price[code], code))
        if not caps:
            raise ValueError("市值一家都算不出來（來源欄位可能改名）")
        caps.sort(reverse=True)
        return [c for _, c in caps[:TOP_N]]
    except Exception as e:
        errors.append(f"市值前百大來源失敗：{e}")
        return None


def fetch_conference(errors: list, top100: list) -> list | None:
    """法說會／財報：MOPS 法人說明會一覽表（本月＋下月，含未來場次）。
    只收市值前百大；擇要訊息驗得出季別＝財報場次，否則當一般法說會。
    回傳 None＝來源失敗（含 top100 為空——「只收前百大」在名單空時等於一場都不收，
    靜默產出空清單會被誤讀成「近兩週沒有財報」，故一律交給呼叫端走退回鏈）"""
    if not top100:
        errors.append("法說會：無市值前百大名單可篩，本輪改用備援來源")
        return None
    top = set(top100)
    today = datetime.now(TPE).date()
    ev = []
    try:
        for k in (0, 1):                       # 本月＋下月；跨年時年份自動進位
            mo = today.month + k
            y, mo = today.year + (mo - 1) // 12, (mo - 1) % 12 + 1
            text = http_bytes(URL_MOPS_CONF, form={
                "encodeURIComponent": "1", "step": "1", "firstin": "1", "off": "1",
                "TYPEK": "sii", "year": str(y - 1911), "month": f"{mo:02d}",
            }).decode("utf-8", "replace")
            for r in mops_rows(text):
                code = r[0]
                if code not in top or len(r) < 6:
                    continue
                # 日期欄有單日「115/07/16」與區間「115/06/30 至 115/07/03」兩種格式，一律取起日
                d = roc_to_date(r[2].split("至")[0].strip())
                if not d:
                    continue
                brief, place = r[5], r[4]
                q = quarter_of(brief)
                note = f"Q{q} 財報" if q else "法說會"
                if "線上" in place or "線上" in brief:
                    note += "（線上）"
                ev.append({"date": d.isoformat(),
                           "type": "earnings" if q else "conference",
                           "code": code, "name": r[1], "note": note})
    except Exception as e:
        errors.append(f"法說會來源（MOPS）失敗：{e}")
        return None
    return ev


def fetch_conference_news(errors: list) -> list | None:
    """法說會備援：從當日重大訊息篩第 12 款（召開法人說明會）。
    只有「當日」公告、抓不到未來場次，故僅在 MOPS 失敗時使用。回傳 None＝來源失敗"""
    ev = []
    try:
        for r in http_json(URL_NEWS):
            clause = str(r.get("符合條款", ""))
            subj = str(r.get("主旨 ") or r.get("主旨") or "")
            body = str(r.get("說明", ""))
            if not re.search(r"第\s*12\s*款", clause) and "法人說明會" not in subj:
                continue
            if "法人說明會" not in subj and "法人說明會" not in body:
                continue
            m = re.search(r"召開法人說明會之日期[：:]\s*(\d{2,3}/\d{1,2}/\d{1,2})", body)
            d = roc_to_date(m.group(1)) if m else roc_to_date(r.get("事實發生日"))
            if not d:
                continue
            note = "法說會"
            if "線上" in subj or "線上" in body[:200]:
                note += "（線上）"
            ev.append({"date": d.isoformat(), "type": "conference",
                       "code": str(r.get("公司代號", "")).strip(),
                       "name": str(r.get("公司名稱", "")).strip(),
                       "note": note})
    except Exception as e:
        errors.append(f"法說會備援來源（重大訊息）失敗：{e}")
        return None
    return ev


RE_STOCK_CODE = re.compile(r"^\d{4}[A-Z]?$")   # 股票／特別股（如 2891B）
RE_ETF_CODE = re.compile(r"^00\d{2,4}$")       # ETF（如 0050、006208）


def is_stock_or_etf(code: str) -> bool:
    """只收股票／ETF 代號，濾掉權證、可轉債等衍生商品（多為 5～6 位數代號）"""
    return bool(RE_STOCK_CODE.match(code) or RE_ETF_CODE.match(code))


def parse_disposition_period(s) -> tuple[str, str] | None:
    """處置期間字串 → (start_iso, end_iso)。相容兩種來源格式：
       115/07/03～115/07/16（全形～，含斜線，TWSE）
       1150710~1150723（半形~，無斜線，TPEx）
       任一段去掉「/」後不是 7 位數字、或轉換失敗 → 回傳 None，呼叫端靜默跳過該列
       （TPEx 無資料時會回「單筆全空白樣板列」而非空陣列，即屬此類，不算錯誤）。"""
    if not s:
        return None
    s = str(s).strip()
    if "～" in s:
        parts = s.split("～", 1)
    elif "~" in s:
        parts = s.split("~", 1)
    else:
        return None
    if len(parts) != 2:
        return None
    out = []
    for p in parts:
        digits = p.strip().replace("/", "")
        if len(digits) != 7 or not digits.isdigit():
            return None
        try:
            out.append(date(int(digits[:3]) + 1911, int(digits[3:5]), int(digits[5:7])).isoformat())
        except ValueError:
            return None
    return out[0], out[1]


def punish_times(raw) -> int:
    """累計處置次數：解析失敗（含 TPEx 無此欄位）一律設 1"""
    try:
        return int(str(raw).strip())
    except (ValueError, TypeError):
        return 1


def fetch_punish(errors: list, old_punish: list | None = None) -> list:
    """處置股：上市（TWSE openapi，固定近期窗口）＋上櫃（TPEx openapi，固定 snapshot），
    兩來源各自獨立失敗、不阻斷。同一檔二度處置時，來源會「第一次＋第二次處置」兩筆
    公告並存（期間重疊、新令取代舊令，2026-07-11 實測 TWSE），故同 code 只保留
    end 最大（最新處置令）那筆；times 取保留筆的 NumberOfAnnouncement 即為累計次數
    （來源會把同檔所有列的該欄位都更新成累計值，實測第一次處置那列也標 2）。
    old_punish：上次成功輸出的 punish 清單，供該段失敗時按 market 沿用。"""
    rows = []
    old_punish = old_punish or []

    twse_ok = False
    try:
        for r in http_json(URL_PUNISH_TWSE) or []:
            code = str(r.get("Code", "")).strip()
            if not is_stock_or_etf(code):
                continue
            period = parse_disposition_period(r.get("DispositionPeriod"))
            if not period:
                continue
            rows.append({"code": code, "name": str(r.get("Name", "")).strip(),
                         "start": period[0], "end": period[1],
                         "times": punish_times(r.get("NumberOfAnnouncement")),
                         "market": "上市"})
        twse_ok = True
    except Exception as e:
        errors.append(f"處置股來源失敗（上市）：{e}")

    tpex_ok = False
    try:
        for r in http_json(URL_PUNISH_TPEX) or []:
            code = str(r.get("SecuritiesCompanyCode", "")).strip()
            if not is_stock_or_etf(code):
                continue
            period = parse_disposition_period(r.get("DispositionPeriod"))
            if not period:
                continue
            rows.append({"code": code, "name": str(r.get("CompanyName", "")).strip(),
                         "start": period[0], "end": period[1],
                         "times": punish_times(r.get("NumberOfAnnouncement")),
                         "market": "上櫃"})
        tpex_ok = True
    except Exception as e:
        errors.append(f"處置股來源失敗（上櫃）：{e}")

    if not twse_ok:
        fallback = [r for r in old_punish if r.get("market") == "上市"]
        if fallback:
            log(f"[處置股] 上市沿用上次資料（{len(fallback)} 筆）")
        rows += fallback
    if not tpex_ok:
        fallback = [r for r in old_punish if r.get("market") == "上櫃"]
        if fallback:
            log(f"[處置股] 上櫃沿用上次資料（{len(fallback)} 筆）")
        rows += fallback

    def period_key(r: dict) -> tuple[str, str]:
        return (str(r["end"]), str(r["start"]))

    best: dict = {}
    for r in rows:
        cur = best.get(r["code"])
        if cur is None or period_key(r) > period_key(cur):
            best[r["code"]] = r
    return sorted(best.values(), key=lambda r: str(r["code"]))


# ─── 行情條與休市日曆 ───────────────────────────────────────

def prev_close_from_chart(result: dict) -> float | None:
    """從日線序列取「上一交易日收盤」；取不到回 None（呼叫端才知道要退回）。

    ※ 不可改用 meta.chartPreviousClose：那是「所請求 range **起點之前**」的收盤，
      range=5d 時＝數個交易日前，漲跌幅會變成多日累計而非當日。
      2026-07-31 實測加權指數：當日 +7.98% 被算成 -1.18%（基準取到 4 個交易日前）。
    盤中 Yahoo 會在「當日 K」之外**另附一筆即時報價**（USDTWD=X 實測：當日 K 的
    close 為 null、另有一筆 08:58 的即時值），同一交易日佔兩個位置，直接取倒數
    第二筆會拿到當日自己。故先用 meta.gmtoffset 換算交易所當地日期歸戶
    （各檔 K 皆落在當地開盤整點，離換日點夠遠），再取最後一個「與當前交易日
    不同日」的收盤。無 timestamp 可對齊時才退回「倒數第二筆有效收盤」。
    """
    quote = ((result.get("indicators") or {}).get("quote") or [{}])[0]
    closes = quote.get("close") or []
    stamps = result.get("timestamp") or []
    if stamps and len(stamps) == len(closes):
        offset = (result.get("meta") or {}).get("gmtoffset") or 0
        rows = [((t + offset) // 86400, float(c))
                for t, c in zip(stamps, closes) if c is not None]
        if rows:
            today = rows[-1][0]
            return next((c for d, c in reversed(rows) if d != today), None)
    valid = [float(c) for c in closes if c is not None]
    return valid[-2] if len(valid) >= 2 else None


def fetch_quotes(errors: list, old_quotes: dict | None = None) -> list:
    """行情條：Yahoo Finance chart API。單檔失敗時若 old_quotes（以 name 為鍵）
    有同名項目則沿用該項，否則才真的跳過；不影響其他檔"""
    old_quotes = old_quotes or {}
    quotes = []
    for symbol, name, kind in QUOTES:
        try:
            url = URL_QUOTE.format(symbol=urllib.parse.quote(symbol))
            j = http_json(url)
            result = (j.get("chart") or {}).get("result") or []
            if not result:
                raise ValueError(f"result 為空（{(j.get('chart') or {}).get('error')}）")
            meta = result[0].get("meta") or {}
            price = meta.get("regularMarketPrice")
            # 上一交易日收盤由 close 序列取（見 prev_close_from_chart 的警告）；
            # 序列不足才退回 chartPreviousClose（此時漲跌幅可能是多日累計）
            prev = prev_close_from_chart(result[0]) or meta.get("chartPreviousClose")
            if price is None:
                raise ValueError("regularMarketPrice 缺值")
            if not prev:
                raise ValueError("前一交易日收盤缺值或為 0")
            price, prev = float(price), float(prev)
            quotes.append({
                "name": name, "kind": kind,
                "price": price, "prev": prev,
                "chg_abs": price - prev,
                "chg_pct": (price / prev - 1) * 100,
                "t": meta.get("regularMarketTime"),
            })
        except Exception as e:
            msg = f"行情來源失敗（{name}／{symbol}）：{e}"
            log(f"[行情] {msg}，跳過")
            errors.append(msg)
            if name in old_quotes:
                quotes.append(old_quotes[name])
                log(f"[行情] {name} 沿用上次資料")
    return quotes


def fetch_sofr(errors: list) -> dict | None:
    """SOFR 3 個月平均利率：NY Fed 官方 API（免金鑰）。
    正常 last/2 即回「最新＋前一營業日」兩筆；若有效筆數不足才改抓 last/5 取最新兩筆。
    抓取失敗、筆數不足兩筆或欄位缺值只印警告回傳 None，呼叫端略過、不阻斷其他行情"""
    try:
        rows = (http_json(URL_SOFR) or {}).get("refRates") or []
        rows = [r for r in rows
                if r.get("effectiveDate") and r.get("average90day") is not None]
        if len(rows) < 2:
            rows = (http_json(URL_SOFR_FALLBACK) or {}).get("refRates") or []
            rows = [r for r in rows
                    if r.get("effectiveDate") and r.get("average90day") is not None]
        rows.sort(key=lambda r: r["effectiveDate"], reverse=True)
        if len(rows) < 2:
            raise ValueError(f"有效資料不足兩筆（{len(rows)} 筆）")
        latest, prev = rows[0], rows[1]
        price, prev_v = float(latest["average90day"]), float(prev["average90day"])
        d = date.fromisoformat(latest["effectiveDate"])
        t = int(datetime(d.year, d.month, d.day, tzinfo=timezone.utc).timestamp())
        return {
            "name": "SOFR 3M", "kind": "yield",
            "price": price, "prev": prev_v,
            "chg_abs": price - prev_v,
            "chg_pct": (price / prev_v - 1) * 100,
            "t": t,
        }
    except Exception as e:
        msg = f"行情來源失敗（SOFR 3M／NY Fed）：{e}"
        log(f"[行情] {msg}，跳過")
        errors.append(msg)
        return None


def fetch_holidays(errors: list) -> list | None:
    """TWSE 休市日曆：只回當年度，跨年由前端週末規則兜底。
    整個來源失敗回傳 None（輸出時省略該鍵），不阻斷其他輸出"""
    try:
        raw = http_json(URL_HOLIDAY)
    except Exception as e:
        log(f"[休市日曆] 來源失敗，略過：{e}")
        errors.append(f"休市日曆來源失敗：{e}")
        return None
    out = set()
    for r in raw or []:
        # 此 API 除休市日外，還混入交易日的說明列（「國曆新年開始交易日」「農曆春節前最後交易日」
        # 等），那些日子照常開盤、不可當休市。判斷兩步（不綁日期）：
        # ① Name 含「開始／最後交易」＝疑似說明列；② 但 Description「第一句」若寫「無交易」，這天本身
        #    是結算交割日（真休市）要留——2021／2022 年官方把結算交割日的 Name 也寫成「農曆春節前最後
        #    交易日」、只在 Description 寫「2月8日市場無交易，僅辦理結算交割作業」。
        # 只看第一句：2023-01-17（交易日）的說明第二句提到 1/18、1/19 無交易，看整段會誤判成休市。
        desc_first = str(r.get("Description", "")).split("。")[0]
        if TRADING_DAY_NOTICE.search(str(r.get("Name", ""))) and "無交易" not in desc_first:
            continue
        s = str(r.get("Date", "")).strip()
        if len(s) != 7 or not s.isdigit():
            continue
        try:
            out.add(date(int(s[:3]) + 1911, int(s[3:5]), int(s[5:7])).isoformat())
        except ValueError:
            continue
    return sorted(out)


# ─── 動態桌布資料：加權盤中走勢／日 K／券資比與融資維持率 ────────

def chart_result(j: dict) -> dict:
    """Yahoo v8 chart 回應 → result[0]；空結果丟 ValueError（附 Yahoo 的 error 內容）"""
    chart = (j or {}).get("chart") or {}
    result = chart.get("result") or []
    if not result:
        raise ValueError(f"result 為空（{chart.get('error')}）")
    return result[0]


def tpe_close_at(d: date) -> datetime:
    return datetime.combine(d, MARKET_CLOSE, tzinfo=TPE)


def hhmm_tpe(ts: int) -> str:
    return datetime.fromtimestamp(ts, TPE).strftime("%H:%M")


def parse_twii_intraday(j: dict, now: datetime) -> dict:
    """加權指數盤中走勢：只取「最後一個完整交易日」＝台北 13:30 已過的最新交易日。
    回傳 {"date": "YYYY-MM-DD", "points": [[epoch秒, 價], ...]}（依時間排序、相鄰 ≤5 分鐘）。

    - 完整與否一律用台北時間判斷（now 轉 TPE；不吃系統時區）。盤中執行時當日不合格、取前一個
      交易日，故部分序列不會取代上一個完整交易日。
    - Yahoo 盤中會在 5 分鐘 K 之外另附一筆即時報價（時間不在 5 分鐘格線上，AGENTS.md 行情段），
      只留落在 5 分鐘格線上的點；13:30:00 整的收盤點在格線上、保留。
    - 該日序列若起點晚於 09:05、終點早於 13:25（Yahoo 還沒追上）或有超過 5 分鐘的缺口，
      一律丟 ValueError（不拿不完整序列冒充完整日，也不偷偷退回更早一天當新資料）。"""
    now = now.astimezone(TPE)
    r = chart_result(j)
    stamps = r.get("timestamp") or []
    closes = ((r.get("indicators") or {}).get("quote") or [{}])[0].get("close") or []
    if not stamps or len(stamps) != len(closes):
        raise ValueError("盤中序列缺 timestamp 或與 close 長度不符")
    days: dict[date, dict[int, float]] = {}
    for t, c in zip(stamps, closes):
        if c is None or t % INTRADAY_STEP:
            continue
        d = datetime.fromtimestamp(t, TPE)
        if not (MARKET_OPEN <= d.time() <= MARKET_CLOSE):
            continue
        days.setdefault(d.date(), {})[int(t)] = round(float(c), 2)
    done = [d for d in days if tpe_close_at(d) <= now]
    if not done:
        raise ValueError("序列內沒有任何已過 13:30 的交易日")
    day = max(done)
    points = sorted(days[day].items())
    first = datetime.fromtimestamp(points[0][0], TPE)
    last = datetime.fromtimestamp(points[-1][0], TPE)
    if first > datetime.combine(day, MARKET_OPEN, tzinfo=TPE) + timedelta(seconds=INTRADAY_STEP):
        raise ValueError(f"{day} 序列起點 {first:%H:%M} 太晚，視為不完整")
    if last < tpe_close_at(day) - timedelta(seconds=INTRADAY_STEP):
        raise ValueError(f"{day} 序列終點 {last:%H:%M} 太早（來源尚未追上收盤），視為不完整")
    for (a, _), (b, _) in pairwise(points):
        if b - a > INTRADAY_STEP:
            raise ValueError(f"{day} 序列在 {hhmm_tpe(a)} 後缺口超過 5 分鐘")
    return {"date": day.isoformat(), "points": [[t, p] for t, p in points]}


def fetch_twii_intraday(wallpaper_errors: list, old: object = None,
                        now: datetime | None = None) -> dict | None:
    """加權盤中走勢鍵。失敗＝沿用 old（含其日期）並把錯誤記到 wallpaper_errors（不是 errors）；
    也不讓日期倒退到比 old 更早。沒有（可用的）old 又失敗回傳 None（輸出時省略該鍵）。
    old 型別不對（不是 dict）一律當沒有；now 供測試注入，預設台北現在時間。"""
    now = now or datetime.now(TPE)
    old = old if isinstance(old, dict) else None
    try:
        new = parse_twii_intraday(http_json(URL_TWII_INTRADAY), now)
    except FETCH_ERRORS as e:
        wallpaper_errors.append(f"加權盤中走勢來源失敗：{e}")
        log(f"[盤中走勢] 本輪失敗，沿用上次資料：{e}")
        return old
    if old and str(old.get("date") or "") > new["date"]:
        return old
    return new


def parse_twii_daily(j: dict, now: datetime) -> list[dict]:
    """加權指數日 K（日期、開高低收），依日期由舊到新、至少 DAILY_MIN 根。
    只收「已收盤」的交易日：當日台北 13:30 前執行時，排除當日那根未收盤的 K。
    OHLC 有任一為 null 的列丟棄（Yahoo 另附的即時報價列常如此）；同日重複取後者。"""
    now = now.astimezone(TPE)
    r = chart_result(j)
    stamps = r.get("timestamp") or []
    q = ((r.get("indicators") or {}).get("quote") or [{}])[0]
    cols = [q.get(k) or [] for k in ("open", "high", "low", "close")]
    if not stamps or any(len(c) != len(stamps) for c in cols):
        raise ValueError("日 K 缺 timestamp 或各欄長度不符")
    rows: dict[date, dict] = {}
    for t, o, h, lo, c in zip(stamps, *cols):
        if None in (o, h, lo, c):
            continue
        d = datetime.fromtimestamp(t, TPE).date()
        if tpe_close_at(d) > now:
            continue
        rows[d] = {"date": d.isoformat(), "open": round(float(o), 2), "high": round(float(h), 2),
                   "low": round(float(lo), 2), "close": round(float(c), 2)}
    out = [rows[d] for d in sorted(rows)]
    if len(out) < DAILY_MIN:
        raise ValueError(f"日 K 只有 {len(out)} 根，不足 {DAILY_MIN} 根")
    return out


def fetch_twii_daily(wallpaper_errors: list, old: object = None,
                     now: datetime | None = None) -> list | None:
    """加權日 K 鍵。失敗＝沿用 old 並記到 wallpaper_errors；沒有（可用的）old 又失敗回傳 None。
    比照盤中走勢，日期不倒退（task 6.4，審查 R3-low）：新抓的最後一根早於 old 的最後一根（Yahoo 偶發回較舊的
    視窗）＝沿用 old 並記錯誤。old 型別不對（不是 list）一律當沒有。"""
    now = now or datetime.now(TPE)
    old = old if isinstance(old, list) else None
    try:
        new = parse_twii_daily(http_json(URL_TWII_DAILY), now)
    except FETCH_ERRORS as e:
        wallpaper_errors.append(f"加權日K來源失敗：{e}")
        log(f"[日K] 本輪失敗，沿用上次資料：{e}")
        return old
    old_last = str(old[-1].get("date") or "") if old and isinstance(old[-1], dict) else ""
    if old_last > new[-1]["date"]:
        wallpaper_errors.append(
            f"加權日K來源回傳較舊的資料（最後一根 {new[-1]['date']} 早於上次的 {old_last}），沿用上次資料")
        log(f"[日K] 來源最後一根 {new[-1]['date']} 早於上次的 {old_last}，沿用上次資料")
        return old
    return new


def margin_int(s) -> int:
    return int(str(s).replace(",", "").strip())


def ymd8_to_date(s) -> date:
    """MI_MARGN 的 date 欄（YYYYMMDD 西元，如 20260930）→ date；格式不對丟 ValueError"""
    m = re.fullmatch(r"(\d{4})(\d{2})(\d{2})", str(s).strip())
    if not m:
        raise ValueError(f"日期格式不是 YYYYMMDD：{s!r}")
    return date(*(int(g) for g in m.groups()))


def margin_ok(j: dict) -> dict:
    if (j or {}).get("stat") != "OK":
        raise ValueError(f"MI_MARGN 回應 stat={(j or {}).get('stat')!r}")
    return j


def parse_margin_summary(j: dict) -> dict:
    """MI_MARGN（selectType=MS 或 ALL 的第一張表）→ 交易日與彙總。
    單位：融資／融券「交易單位」＝張；融資金額「仟元」。採「今日餘額」欄（規格算 2.53% 用的欄位；
    表尾註記說以次日的「前日餘額」為準，但那要等下一個交易日才有，與「當日收盤價」對不上）。"""
    j = margin_ok(j)
    day = ymd8_to_date(j["date"])
    t = j["tables"][0]
    col = t["fields"].index("今日餘額")
    by_label = {str(r[0]).strip(): r for r in t["data"]}
    return {
        "date": day.isoformat(),
        "margin_lots": margin_int(by_label["融資(交易單位)"][col]),
        "short_lots": margin_int(by_label["融券(交易單位)"][col]),
        "margin_amount_k": margin_int(by_label["融資金額(仟元)"][col]),
    }


def parse_margin_stocks(j: dict) -> tuple[str, list[tuple[str, int, int]]]:
    """MI_MARGN selectType=ALL → (交易日, [(代號, 融資今日餘額張, 融券今日餘額張), ...])。
    個股表的「今日餘額」欄出現兩次：第一次是融資、第二次是融券（依欄位順序取，不寫死位置）。"""
    j = margin_ok(j)
    day = ymd8_to_date(j["date"]).isoformat()
    for t in j["tables"]:
        fields = t.get("fields") or []
        cols = [i for i, f in enumerate(fields) if f == "今日餘額"]
        if fields[:1] == ["代號"] and len(cols) == 2:
            return day, [(str(r[0]).strip(), margin_int(r[cols[0]]), margin_int(r[cols[1]]))
                         for r in t["data"]]
    raise ValueError("MI_MARGN 回應找不到個股融資融券明細表")


def parse_day_all_prices(rows: list) -> tuple[str, dict[str, float]]:
    """STOCK_DAY_ALL → (價格所屬交易日, {代號: 收盤價})。Date 欄為民國 7 碼（如 1151001），
    全表須同一天；ClosingPrice 空字串／非數字／≤0（停牌、無成交）＝查無收盤價，不收。"""
    days = {roc_to_date(r.get("Date")) for r in rows}
    (day,) = days if len(days) == 1 else (None,)
    if day is None:
        raise ValueError(f"STOCK_DAY_ALL 的日期欄不是單一有效日期：{sorted(map(str, days))[:3]}")
    prices: dict[str, float] = {}
    for r in rows:
        try:
            v = float(str(r.get("ClosingPrice") or "").replace(",", "").strip())
        except ValueError:
            continue
        if v > 0:
            prices[str(r.get("Code") or "").strip()] = v
    return day.isoformat(), prices


def compute_margin(summary: dict, stocks: list, prices: dict[str, float]) -> dict:
    """券資比＝融券餘額張 ÷ 融資餘額張 × 100；
    融資維持率＝Σ(個股融資餘額張 × 1000 × 收盤價) ÷ (融資金額仟元 × 1000) × 100。
    查無收盤價的個股略過（列在 unpriced），ETF 與一般股票同樣計入。"""
    margin_lots, short_lots = summary["margin_lots"], summary["short_lots"]
    amount_yuan = summary["margin_amount_k"] * 1000
    if margin_lots <= 0 or amount_yuan <= 0:
        raise ValueError(f"融資餘額為 {margin_lots} 張／{amount_yuan} 元，無法計算")
    detail = sum(x[1] for x in stocks)
    if abs(detail - margin_lots) > margin_lots * MARGIN_SUM_TOLERANCE:
        raise ValueError(f"個股融資張數加總 {detail} 與彙總 {margin_lots} 差距過大，疑似欄位位移")
    value, unpriced = 0.0, []
    for code, lots, _short in stocks:
        p = prices.get(code)
        if p is None:
            unpriced.append(code)
            continue
        value += lots * 1000 * p
    return {
        "date": summary["date"],
        "margin_lots": margin_lots,
        "short_lots": short_lots,
        "short_margin_ratio": round(short_lots / margin_lots * 100, 2),
        "maintenance_ratio": round(value / amount_yuan * 100, 2),
        "unpriced": unpriced,
    }


def fetch_margin(wallpaper_errors: list, old: object = None) -> dict | None:
    """上市券資比與融資維持率鍵。流程：
    1. 取最新已公布的彙總（MS，不帶 date）→ 得交易日 D；與 old 同日就不再下載大表，直接沿用。
    2. 以 D 取個股明細（ALL，帶 date=D，避免兩次請求跨過公布時點）與 STOCK_DAY_ALL 收盤價。
    3. 收盤價日期 ≠ D（例如 15:00 那輪價格已是當日、融資仍是前一日）＝不得混用，沿用 old，
       不算錯誤（晚間公布前屬正常）。
    任何抓取／解析失敗＝沿用 old 並記到 wallpaper_errors；沒有（可用的）old 又無法算出時回傳 None。
    old 型別不對（不是 dict）一律當沒有。"""
    old = old if isinstance(old, dict) else None
    try:
        summary = parse_margin_summary(http_json(URL_MARGN.format(kind="MS")))
        day = summary["date"]
        if old and old.get("date") == day:
            return old
        d8 = day.replace("-", "")
        all_day, stocks = parse_margin_stocks(
            http_json(URL_MARGN.format(kind="ALL") + f"&date={d8}"))
        if all_day != day:
            raise ValueError(f"彙總日期 {day} 與個股明細日期 {all_day} 不一致")
        price_day, prices = parse_day_all_prices(http_json(URL_DAY_ALL))
        if price_day != day:
            log(f"[券資比] 收盤價日期 {price_day} ≠ 融資日期 {day}，不混用，沿用上次資料")
            return old
        return compute_margin(summary, stocks, prices)
    except FETCH_ERRORS as e:
        wallpaper_errors.append(f"券資比／融資維持率來源失敗：{e}")
        log(f"[券資比] 本輪失敗，沿用上次資料：{e}")
        return old


def guarded_wallpaper_fetch(label: str, fetch, wallpaper_errors: list, old: object,
                            old_type: type, *args):
    """動態桌布新鍵的最後一道防線（task 6.4，審查 R3-M）：抓取函式即使丟出 FETCH_ERRORS 以外的例外，
    也只沿用 old（型別不對＝沒有）並記到 wallpaper_errors——新鍵的失敗不得讓整輪不寫檔、拖垮既有鍵
    （spec「新資料鍵失敗時沿用舊值……不影響其他資料鍵的更新」）。"""
    try:
        return fetch(wallpaper_errors, old, *args)
    except Exception as e:  # noqa: BLE001 — 新鍵的任何失敗都不得拖垮既有鍵的更新與寫檔
        wallpaper_errors.append(f"{label}來源發生非預期錯誤：{type(e).__name__}: {e}")
        log(f"[{label}] 非預期錯誤，沿用上次資料：{e!r}")
        return old if isinstance(old, old_type) else None


# ─── 主流程 ─────────────────────────────────────────────────

def lively_wallpaper_dirs() -> list[Path]:
    """動態尋找 Lively 桌布資料夾。Lively 會把桌布整包複製到自身 Library（含 wptmp 暫存與
    正式安裝兩種位置），且其 WebView2 擋掉絕對 file:// 跨資料夾讀取——桌布只能讀「自身資料夾」
    那份 tw_events.js，故資料層負責把最新資料一併原子寫到這些複製位置。動態尋找＝不寫死隨機碼
    路徑，重匯入自動跟上。找不到（Lively 沒裝／沒此桌布）＝回空清單、靜默略過。"""
    la = os.environ.get("LOCALAPPDATA")
    if not la:
        return []
    pat = ("12030rocksdanister.LivelyWallpaper_*/LocalCache/Local/"
           "Lively Wallpaper/Library/**/finance-calendar.html")
    try:
        return sorted({p.parent for p in (Path(la) / "Packages").glob(pat)})
    except OSError:
        return []


def main(now: datetime | None = None) -> int:
    if isinstance(sys.stdout, io.TextIOWrapper):
        try:
            sys.stdout.reconfigure(encoding="utf-8", errors="replace")
        except Exception:
            pass

    errors: list = []

    # 開機排程補跑時網路常還沒就緒；先等網路，逾時就放行，靠下面的舊資料
    # fallback 兜底（不讓「網路未就緒」變成把輸出檔洗成空資料）
    if not wait_for_network():
        errors.append("啟動時網路等待逾時，改用既有資料兜底")

    # 讀舊輸出供本輪各來源失敗時 fallback；不存在或壞掉就當作沒有舊資料
    old: dict = {}
    try:
        old_path = Path(__file__).resolve().parent / "tw_events.json"
        old = json.loads(old_path.read_text(encoding="utf-8"))
    except Exception:
        old = {}

    # 窗口錨定台北市場日曆（date.today() 吃系統時區：人在東京時 00:00–01:00 JST
    # 會把窗口推前一日、濾掉台北當日事件——排程 00:00 JST 那輪必踩）
    today = datetime.now(TPE).date()
    start = today - timedelta(days=INCLUDE_PAST_DAYS)
    end = today + timedelta(days=WINDOW_DAYS)

    log(f"抓取總經日曆＋台股動態事件＋行情＋休市日曆（{start} ~ {end}）…")

    # fetched 時間戳：本輪至少一個來源抓到新資料才刷新。punish／quotes 內部自帶
    # fallback、由回傳值判別不出新舊，近似取可判別的六個來源；全斷網情境仍準確
    any_fresh = False

    macro = fetch_macro(errors)
    any_fresh = any_fresh or macro is not None
    if macro is None:
        log("[總經] 本輪失敗，沿用上次資料")
        macro = old.get("macro") or []

    old_events = old.get("events") or []
    def _old_events_of(t: str) -> list:
        return [e for e in old_events if e.get("type") == t]

    div_ev = fetch_dividend(errors)
    any_fresh = any_fresh or div_ev is not None
    if div_ev is None:
        log("[除權息] 本輪失敗，沿用上次資料")
        div_ev = _old_events_of("dividend")
    meeting_ev = fetch_meeting(errors)
    any_fresh = any_fresh or meeting_ev is not None
    if meeting_ev is None:
        log("[股東會] 本輪失敗，沿用上次資料")
        meeting_ev = _old_events_of("meeting")
    # 市值前百大：排名變動極慢，每日只算一次（同日沿用快取，省 1.6 MB 下載）
    from_cache = bool(old.get("top100")) and old.get("top100_date") == today.isoformat()
    top100 = fetch_top100(errors, old)
    if top100 is None:
        # 沿用舊名單時 top100_date 必須保留舊值，否則會被當成今天已算過而整天不再重試
        log("[前百大] 本輪失敗，沿用上次名單")
        top100, top100_date = (old.get("top100") or []), old.get("top100_date")
    else:
        top100_date = today.isoformat()
        any_fresh = any_fresh or not from_cache   # 走快取沒發請求，不算「抓到新資料」

    # 主來源 MOPS 一覽表（含未來場次）→ 失敗退回當日重大訊息 → 再失敗才沿用上次
    conf_ev = fetch_conference(errors, top100)
    if conf_ev is None:
        log("[法說會] MOPS 來源失敗，改用重大訊息備援")
        conf_ev = fetch_conference_news(errors)
    any_fresh = any_fresh or conf_ev is not None
    if conf_ev is None:
        log("[法說會] 本輪失敗，沿用上次資料")
        conf_ev = _old_events_of("conference") + _old_events_of("earnings")
    raw = div_ev + meeting_ev + conf_ev

    punish = fetch_punish(errors, old_punish=old.get("punish"))

    old_quotes = {q.get("name"): q for q in (old.get("quotes") or []) if q.get("name")}
    quotes = fetch_quotes(errors, old_quotes=old_quotes)
    sofr = fetch_sofr(errors)
    any_fresh = any_fresh or sofr is not None
    if sofr is None and "SOFR 3M" in old_quotes:
        log("[行情] SOFR 3M 沿用上次資料")
        sofr = old_quotes["SOFR 3M"]
    if sofr:
        quotes.append(sofr)

    holidays = fetch_holidays(errors)
    any_fresh = any_fresh or holidays is not None
    if holidays is None and old.get("holidays") is not None:
        log("[休市日曆] 本輪失敗，沿用上次資料")
        holidays = old.get("holidays")

    # 動態桌布三鍵：各自獨立失敗沿用上次輸出（含其日期）；全新安裝又失敗則省略該鍵。
    # 失敗記到獨立的 wallpaper_errors（不進 errors：Lively 頁腳會顯示 errors 筆數）；
    # 也不參與 any_fresh（否則只要 Yahoo 通，fetched 就每輪刷新，蓋掉 Lively 的「已 N 天未更新」）
    wallpaper_errors: list = []
    now_tpe = now or datetime.now(TPE)
    twii_intraday = guarded_wallpaper_fetch(
        "加權盤中走勢", fetch_twii_intraday, wallpaper_errors, old.get("twii_intraday"), dict, now_tpe)
    twii_daily = guarded_wallpaper_fetch(
        "加權日K", fetch_twii_daily, wallpaper_errors, old.get("twii_daily"), list, now_tpe)
    margin = guarded_wallpaper_fetch(
        "券資比／融資維持率", fetch_margin, wallpaper_errors, old.get("margin"), dict)

    # 台股事件：過濾時間窗＋去重
    seen, events = set(), []
    for e in raw:
        d = date.fromisoformat(e["date"])
        if not (start <= d <= end):
            continue
        key = (e["type"], e["code"], e["date"])
        if key in seen:
            continue
        seen.add(key)
        events.append(e)
    events.sort(key=lambda e: (e["date"], e["type"], e["code"]))

    counts = {t: sum(1 for e in events if e["type"] == t)
              for t in ("dividend", "meeting", "conference", "earnings")}
    counts["macro"] = len(macro)
    counts["quotes"] = len(quotes)
    counts["punish"] = len(punish)

    now_str = datetime.now().strftime("%Y-%m-%d %H:%M")
    payload = {
        "updated": now_str,
        # 最後一次「真的抓到新資料」的時間；全來源失敗時沿用舊值，供前端算資料多舊
        "fetched": now_str if any_fresh else (old.get("fetched") or old.get("updated") or now_str),
        "window": {"start": start.isoformat(), "end": end.isoformat()},
        "counts": counts,
        "errors": errors,
        "wallpaper_errors": wallpaper_errors,
        "sources": {
            "macro": "ForexFactory 週曆(本週+下週)",
            "dividend": "TWSE 除權除息預告表(TWT48U)",
            "meeting": "TWSE OpenAPI t187ap41_L",
            "conference": "MOPS 法人說明會一覽表 t100sb02_1（備援：TWSE OpenAPI t187ap04_L 第12款）",
            "earnings": "同 conference，市值前百大且擇要訊息驗得出季別者",
            "punish": "TWSE OpenAPI announcement/punish + TPEx OpenAPI tpex_disposal_information",
            "quotes": "Yahoo Finance chart API + NY Fed SOFR",
            "holidays": "TWSE OpenAPI holidaySchedule",
            "twii_intraday": "Yahoo Finance chart API ^TWII interval=5m&range=5d（最後一個完整交易日）",
            "twii_daily": "Yahoo Finance chart API ^TWII interval=1d&range=3mo（已收盤交易日）",
            "margin": "TWSE rwd MI_MARGN（MS 彙總＋ALL 個股）＋ OpenAPI STOCK_DAY_ALL 收盤價",
        },
        "macro_meta": {"countries": "美國・歐元區・日本", "importance": "中高重要性"},
        "macro": macro,
        "events": events,
        "punish": punish,
        "quotes": quotes,
        "top100": top100,            # 市值前百大代號；每日重算一次，供下輪判斷是否走快取
        "top100_date": top100_date,
    }
    if holidays is not None:
        payload["holidays"] = holidays
    # 動態桌布新鍵（格式見 AGENTS.md「現況」資料層一節）；既有鍵的格式與語意不變
    if twii_intraday is not None:
        payload["twii_intraday"] = twii_intraday
    if twii_daily is not None:
        payload["twii_daily"] = twii_daily
    if margin is not None:
        payload["margin"] = margin

    out_dirs = [Path(__file__).resolve().parent]
    out_dirs += [Path(a) for a in sys.argv[1:]]
    out_dirs += lively_wallpaper_dirs()   # 鏡像到 Lively 桌布資料夾（其 WebView2 擋跨資料夾讀取）
    js_text = "window.TW_EVENTS = " + json.dumps(payload, ensure_ascii=False) + ";\n"
    json_text = json.dumps(payload, ensure_ascii=False, indent=1)

    ok = 0
    for d in out_dirs:
        try:
            d.mkdir(parents=True, exist_ok=True)
            json_path, js_path = d / "tw_events.json", d / "tw_events.js"
            json_tmp = json_path.with_suffix(json_path.suffix + ".tmp")
            js_tmp = js_path.with_suffix(js_path.suffix + ".tmp")
            json_tmp.write_text(json_text, encoding="utf-8")
            js_tmp.write_text(js_text, encoding="utf-8")
            os.replace(json_tmp, json_path)   # 原子寫檔：中途失敗不會留半份輸出
            os.replace(js_tmp, js_path)
            log(f"已輸出 → {d}")
            ok += 1
        except Exception as e:
            log(f"輸出到 {d} 失敗：{e}")

    h_desc = f"{len(holidays)} 筆" if holidays is not None else "抓取失敗"
    quote_total = len(QUOTES) + 1   # +1＝SOFR 3M（NY Fed，不在 QUOTES 清單內）
    punish_twse = sum(1 for p in punish if p["market"] == "上市")
    punish_tpex = sum(1 for p in punish if p["market"] == "上櫃")
    log(f"完成：總經 {counts['macro']}、除權息 {counts['dividend']}、"
        f"股東會 {counts['meeting']}、財報 {counts['earnings']}、"
        f"法說會 {counts['conference']} 筆、"
        f"處置 {counts['punish']} 筆（上市 {punish_twse}／上櫃 {punish_tpex}）、"
        f"行情 {counts['quotes']}/{quote_total}、休市日曆 {h_desc}"
        + (f"；警告 {len(errors)} 項：{'；'.join(errors)}" if errors else "")
        + (f"；桌布資料警告 {len(wallpaper_errors)} 項：{'；'.join(wallpaper_errors)}"
           if wallpaper_errors else ""))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
