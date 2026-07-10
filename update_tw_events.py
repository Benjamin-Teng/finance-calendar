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
  法說會   TWSE OpenAPI 重大訊息（篩「第12款＝召開法人說明會」）
           https://openapi.twse.com.tw/v1/opendata/t187ap04_L
  （FinMind 需逐檔查詢無法一次撈全市場，故以 TWSE 為主。）

用法：
  python update_tw_events.py
      → 輸出到本腳本所在資料夾（即 Wallpaper Engine 專案資料夾）
  python update_tw_events.py "D:\\其他資料夾"
      → 額外同步輸出到指定資料夾（可多個）
"""

import json
import re
import ssl
import sys
import urllib.request
from datetime import date, datetime, timedelta, timezone
from pathlib import Path

# ─── 可調參數 ────────────────────────────────────────────────
WINDOW_DAYS = 14                       # 台股動態事件：今天 ～ 今天+14 天
INCLUDE_PAST_DAYS = 0                  # 想保留剛發生過的 N 天可調大
MACRO_CURRENCIES = ("USD", "EUR", "JPY")   # 總經：美元、歐元區、日圓
MACRO_IMPACTS = ("High", "Medium")         # 總經重要性：高、中
TIMEOUT = 30
# ────────────────────────────────────────────────────────────

UA = {"User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) finance-calendar/1.0"}
TPE = timezone(timedelta(hours=8))     # 台北（無夏令時間，固定 +8）

URL_FF_THIS = "https://nfs.faireconomy.media/ff_calendar_thisweek.json"
URL_FF_NEXT = "https://nfs.faireconomy.media/ff_calendar_nextweek.json"
URL_DIV_RWD  = "https://www.twse.com.tw/rwd/zh/exRight/TWT48U?response=json"
URL_DIV_OAPI = "https://openapi.twse.com.tw/v1/exchangeReport/TWT48U_ALL"
URL_MEETING  = "https://openapi.twse.com.tw/v1/opendata/t187ap41_L"
URL_NEWS     = "https://openapi.twse.com.tw/v1/opendata/t187ap04_L"

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


def http_json(url: str):
    req = urllib.request.Request(url, headers=UA)
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
            return json.loads(r.read().decode("utf-8"))
    except ssl.SSLError:
        ctx = ssl.create_default_context()
        ctx.check_hostname = False
        ctx.verify_mode = ssl.CERT_NONE
        with urllib.request.urlopen(req, timeout=TIMEOUT, context=ctx) as r:
            return json.loads(r.read().decode("utf-8"))


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

def fetch_macro(errors: list) -> list:
    rows = []
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

def fetch_dividend(errors: list) -> list:
    """除權息：TWSE RWD（即時）→ 失敗改 openapi"""
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
    except Exception as e:
        errors.append(f"除權息來源失敗：{e}")
    return ev


def fetch_meeting(errors: list) -> list:
    """股東會（常會＋臨時會）"""
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
    return ev


def fetch_conference(errors: list) -> list:
    """法說會：從重大訊息中篩第 12 款（召開法人說明會）"""
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
        errors.append(f"法說會來源失敗：{e}")
    return ev


# ─── 主流程 ─────────────────────────────────────────────────

def main() -> int:
    try:
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    except Exception:
        pass

    today = date.today()
    start = today - timedelta(days=INCLUDE_PAST_DAYS)
    end = today + timedelta(days=WINDOW_DAYS)
    errors: list = []

    log(f"抓取總經日曆＋台股動態事件（{start} ~ {end}）…")
    macro = fetch_macro(errors)
    raw = fetch_dividend(errors) + fetch_meeting(errors) + fetch_conference(errors)

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
              for t in ("dividend", "meeting", "conference")}
    counts["macro"] = len(macro)

    payload = {
        "updated": datetime.now().strftime("%Y-%m-%d %H:%M"),
        "window": {"start": start.isoformat(), "end": end.isoformat()},
        "counts": counts,
        "errors": errors,
        "sources": {
            "macro": "ForexFactory 週曆(本週+下週)",
            "dividend": "TWSE 除權除息預告表(TWT48U)",
            "meeting": "TWSE OpenAPI t187ap41_L",
            "conference": "TWSE OpenAPI t187ap04_L(第12款)",
        },
        "macro_meta": {"countries": "美國・歐元區・日本", "importance": "中高重要性"},
        "macro": macro,
        "events": events,
    }

    out_dirs = [Path(__file__).resolve().parent]
    out_dirs += [Path(a) for a in sys.argv[1:]]
    js_text = "window.TW_EVENTS = " + json.dumps(payload, ensure_ascii=False) + ";\n"
    json_text = json.dumps(payload, ensure_ascii=False, indent=1)

    ok = 0
    for d in out_dirs:
        try:
            d.mkdir(parents=True, exist_ok=True)
            (d / "tw_events.json").write_text(json_text, encoding="utf-8")
            (d / "tw_events.js").write_text(js_text, encoding="utf-8")
            log(f"已輸出 → {d}")
            ok += 1
        except Exception as e:
            log(f"輸出到 {d} 失敗：{e}")

    log(f"完成：總經 {counts['macro']}、除權息 {counts['dividend']}、"
        f"股東會 {counts['meeting']}、法說會 {counts['conference']} 筆"
        + (f"；警告 {len(errors)} 項：{'；'.join(errors)}" if errors else ""))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
