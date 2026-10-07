"""動態桌布資料層（update_tw_events.py 新增的三個輸出鍵）的單元測試。

執行：uv run --no-project python -m unittest discover -s tests -v
全部用 tests/fixtures/ 的真實錄製回應（來源與錄製時間見該目錄 README.md），
「現在時間」一律由測試注入，不依賴真實時鐘，也不發任何網路請求。
"""

import contextlib
import copy
import http.client
import json
import sys
import tempfile
import unittest
from datetime import date, datetime, timedelta, timezone
from itertools import pairwise
from pathlib import Path
from typing import ClassVar
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

import update_tw_events as m

FIX = Path(__file__).resolve().parent / "fixtures"
TPE = timezone(timedelta(hours=8))


def load(name: str):
    return json.loads((FIX / name).read_text(encoding="utf-8"))


def tpe(y: int, mo: int, d: int, h: int = 0, mi: int = 0, s: int = 0) -> datetime:
    return datetime(y, mo, d, h, mi, s, tzinfo=TPE)


def chart_with_filter(j: dict, keep) -> dict:
    """依 keep(台北時間 datetime) 過濾 Yahoo chart 回應的 timestamp 與各序列（用來模擬「盤中那輪」）"""
    out = copy.deepcopy(j)
    r = out["chart"]["result"][0]
    stamps = r["timestamp"]
    idx = [i for i, t in enumerate(stamps) if keep(datetime.fromtimestamp(t, TPE))]
    r["timestamp"] = [stamps[i] for i in idx]
    q = r["indicators"]["quote"][0]
    for k in list(q):
        q[k] = [q[k][i] for i in idx]
    return out


def hhmm(ts: int) -> str:
    return datetime.fromtimestamp(ts, TPE).strftime("%H:%M")


class TwiiIntradayTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.chart = load("yahoo_twii_5m_5d.json")

    def assert_valid_series(self, res: dict, day: str):
        self.assertEqual(res["date"], day)
        pts = res["points"]
        stamps = [p[0] for p in pts]
        self.assertEqual(stamps, sorted(stamps), "序列要依時間排序")
        self.assertEqual(len(stamps), len(set(stamps)), "時間點不得重複")
        for a, b in pairwise(stamps):
            self.assertLessEqual(b - a, 300, "相鄰間隔不得超過 5 分鐘")
        for ts in stamps:
            d = datetime.fromtimestamp(ts, TPE)
            self.assertEqual(d.date().isoformat(), day, "不得混入別天的點")
            self.assertTrue(9 <= d.hour < 14 or (d.hour == 13 and d.minute <= 30))
        self.assertEqual(hhmm(stamps[0]), "09:00")
        self.assertGreaterEqual(hhmm(stamps[-1]), "13:25")

    def test_after_close_picks_that_day(self):
        # 15:00 那輪：10/1 已過 13:30 → 當日、日期為當日
        res = m.parse_twii_intraday(self.chart, tpe(2026, 10, 1, 15, 0))
        self.assert_valid_series(res, "2026-10-01")
        self.assertEqual(len(res["points"]), 54)
        self.assertAlmostEqual(res["points"][-1][1], 48281.21, places=2)

    def test_recorded_partial_today_is_ignored(self):
        # 樣本錄於 10/2 10:50，當日只有 09:00–10:27 的未收盤序列＋一筆即時報價
        res = m.parse_twii_intraday(self.chart, tpe(2026, 10, 2, 10, 50))
        self.assert_valid_series(res, "2026-10-01")

    def test_noon_run_does_not_overwrite(self):
        # 10/1 中午 12:00 那輪：Yahoo 只有 10/1 到 12:00 的部分序列（用真實序列截斷模擬）
        partial = chart_with_filter(
            self.chart,
            lambda d: d.date() <= date(2026, 10, 1)
            and not (d.date() == date(2026, 10, 1) and d.hour * 60 + d.minute > 12 * 60),
        )
        old = m.parse_twii_intraday(self.chart, tpe(2026, 9, 30, 15, 0))
        self.assertEqual(old["date"], "2026-09-30")
        errors: list = []
        with mock.patch.object(m, "http_json", return_value=partial):
            res = m.fetch_twii_intraday(errors, old, tpe(2026, 10, 1, 12, 0))
        assert res is not None
        self.assertEqual(res, old, "中午那輪不可用未收盤序列取代前一個完整交易日")
        self.assertEqual(res["date"], "2026-09-30")
        self.assertEqual(errors, [])

    def test_afternoon_run_updates(self):
        old = m.parse_twii_intraday(self.chart, tpe(2026, 9, 30, 15, 0))
        errors: list = []
        with mock.patch.object(m, "http_json", return_value=self.chart):
            res = m.fetch_twii_intraday(errors, old, tpe(2026, 10, 1, 15, 0))
        assert res is not None
        self.assertEqual(res["date"], "2026-10-01")
        self.assertNotEqual(res, old)
        self.assertEqual(errors, [])

    def test_failure_keeps_old_and_records_error(self):
        old = m.parse_twii_intraday(self.chart, tpe(2026, 9, 30, 15, 0))
        errors: list = []
        with mock.patch.object(m, "http_json", side_effect=OSError("連線逾時")):
            res = m.fetch_twii_intraday(errors, old, tpe(2026, 10, 1, 15, 0))
        self.assertIs(res, old)
        self.assertEqual(len(errors), 1)
        self.assertIn("連線逾時", errors[0])

    def test_failure_without_old_returns_none(self):
        errors: list = []
        with mock.patch.object(m, "http_json", side_effect=OSError("x")):
            self.assertIsNone(m.fetch_twii_intraday(errors, None, tpe(2026, 10, 1, 15, 0)))
        self.assertEqual(len(errors), 1)

    def test_unparsable_response_keeps_old(self):
        old = m.parse_twii_intraday(self.chart, tpe(2026, 9, 30, 15, 0))
        errors: list = []
        bad = {"chart": {"result": None, "error": {"code": "Not Found"}}}
        with mock.patch.object(m, "http_json", return_value=bad):
            res = m.fetch_twii_intraday(errors, old, tpe(2026, 10, 1, 15, 0))
        self.assertIs(res, old)
        self.assertEqual(len(errors), 1)

    def test_live_quote_point_is_excluded(self):
        # Yahoo 會在 K 之外另附一筆即時報價（時間不在 5 分鐘格線上），不可混入序列
        j = copy.deepcopy(self.chart)
        r = j["chart"]["result"][0]
        last_1001 = max(
            t for t in r["timestamp"] if datetime.fromtimestamp(t, TPE).date() == date(2026, 10, 1)
        )
        live = last_1001 + 5 * 60 - 3     # 13:29:57，離格線 3 秒
        i = r["timestamp"].index(last_1001) + 1
        r["timestamp"].insert(i, live)
        for k, v in r["indicators"]["quote"][0].items():
            v.insert(i, 99999.0 if k == "close" else 0)
        res = m.parse_twii_intraday(j, tpe(2026, 10, 1, 15, 0))
        self.assertNotIn(live, [p[0] for p in res["points"]])
        self.assert_valid_series(res, "2026-10-01")
        self.assertNotIn(99999.0, [p[1] for p in res["points"]])

    def test_close_auction_point_at_1330_is_kept(self):
        j = copy.deepcopy(self.chart)
        r = j["chart"]["result"][0]
        last_1001 = max(
            t for t in r["timestamp"] if datetime.fromtimestamp(t, TPE).date() == date(2026, 10, 1)
        )
        i = r["timestamp"].index(last_1001) + 1
        r["timestamp"].insert(i, last_1001 + 300)       # 13:30:00
        for k, v in r["indicators"]["quote"][0].items():
            v.insert(i, 48353.49 if k == "close" else 0)
        res = m.parse_twii_intraday(j, tpe(2026, 10, 1, 15, 0))
        self.assertEqual(hhmm(res["points"][-1][0]), "13:30")
        self.assertEqual(res["points"][-1][1], 48353.49)

    def test_null_close_points_are_dropped(self):
        j = copy.deepcopy(self.chart)
        r = j["chart"]["result"][0]
        # 把 10/1 的最後一根改成 null：序列只剩到 13:20，13:20 → 結束缺口 → 判不完整
        last_1001 = max(
            t for t in r["timestamp"] if datetime.fromtimestamp(t, TPE).date() == date(2026, 10, 1)
        )
        r["indicators"]["quote"][0]["close"][r["timestamp"].index(last_1001)] = None
        with self.assertRaises(ValueError):
            m.parse_twii_intraday(j, tpe(2026, 10, 1, 15, 0))

    def test_stale_series_after_close_is_incomplete(self):
        # 10/2 14:00：Yahoo 的 10/2 序列只到 10:27（還沒追上）→ 不可當完整日，也不可退回舊日當新資料
        with self.assertRaises(ValueError):
            m.parse_twii_intraday(self.chart, tpe(2026, 10, 2, 14, 0))
        old = m.parse_twii_intraday(self.chart, tpe(2026, 10, 1, 15, 0))
        errors: list = []
        with mock.patch.object(m, "http_json", return_value=self.chart):
            res = m.fetch_twii_intraday(errors, old, tpe(2026, 10, 2, 14, 0))
        self.assertIs(res, old)
        self.assertEqual(len(errors), 1)

    def test_gap_over_five_minutes_is_rejected(self):
        j = copy.deepcopy(self.chart)
        r = j["chart"]["result"][0]
        victim = next(
            t for t in r["timestamp"]
            if datetime.fromtimestamp(t, TPE) == tpe(2026, 10, 1, 11, 0)
        )
        r["indicators"]["quote"][0]["close"][r["timestamp"].index(victim)] = None
        with self.assertRaises(ValueError):
            m.parse_twii_intraday(j, tpe(2026, 10, 1, 15, 0))

    def test_weekend_uses_last_complete_trading_day(self):
        # 週六執行：最後完整交易日＝週五 10/2，但樣本的 10/2 未收完 → 判不完整；
        # 改用只含完整日的子集驗證「週末取最近交易日」
        full = chart_with_filter(self.chart, lambda d: d.date() <= date(2026, 10, 1))
        res = m.parse_twii_intraday(full, tpe(2026, 10, 3, 9, 0))
        self.assert_valid_series(res, "2026-10-01")

    def test_does_not_regress_to_older_day_than_old(self):
        newer = {"date": "2026-10-02", "points": [[1, 1.0]]}
        errors: list = []
        with mock.patch.object(m, "http_json", return_value=self.chart):
            res = m.fetch_twii_intraday(errors, newer, tpe(2026, 10, 1, 15, 0))
        self.assertIs(res, newer)

    def test_uses_taipei_clock_not_system_timezone(self):
        # 同一個瞬間用 UTC 表示（10/1 05:00Z＝台北 13:00，未收盤）→ 仍應判 10/1 未完整、取 9/30
        now_utc = datetime(2026, 10, 1, 5, 0, tzinfo=timezone.utc)
        res = m.parse_twii_intraday(self.chart, now_utc)
        self.assertEqual(res["date"], "2026-09-30")
        now_utc2 = datetime(2026, 10, 1, 5, 30, tzinfo=timezone.utc)   # 台北 13:30 整
        self.assertEqual(m.parse_twii_intraday(self.chart, now_utc2)["date"], "2026-10-01")


class TwiiDailyTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.chart = load("yahoo_twii_1d_3mo.json")

    def test_enough_sorted_and_last_is_latest_closed_day(self):
        res = m.parse_twii_daily(self.chart, tpe(2026, 10, 2, 10, 50))
        self.assertGreaterEqual(len(res), 40)
        dates = [c["date"] for c in res]
        self.assertEqual(dates, sorted(dates))
        self.assertEqual(len(dates), len(set(dates)))
        # 盤中執行：排除當日（10/2）未收盤那根，最後一筆＝10/1
        self.assertEqual(dates[-1], "2026-10-01")
        last = res[-1]
        self.assertEqual(set(last), {"date", "open", "high", "low", "close"})
        self.assertAlmostEqual(last["close"], 48353.49, places=2)
        self.assertAlmostEqual(res[-2]["close"], 47940.13, places=2)    # 9/30 收盤，與 MI_INDEX 一致

    def test_includes_today_after_close(self):
        res = m.parse_twii_daily(self.chart, tpe(2026, 10, 2, 14, 0))
        self.assertEqual(res[-1]["date"], "2026-10-02")
        self.assertEqual(res[-2]["date"], "2026-10-01")

    def test_excludes_today_exactly_before_1330(self):
        res = m.parse_twii_daily(self.chart, tpe(2026, 10, 2, 13, 29))
        self.assertEqual(res[-1]["date"], "2026-10-01")
        res = m.parse_twii_daily(self.chart, tpe(2026, 10, 2, 13, 30))
        self.assertEqual(res[-1]["date"], "2026-10-02")

    def test_null_rows_are_dropped_and_duplicates_keep_last(self):
        j = copy.deepcopy(self.chart)
        r = j["chart"]["result"][0]
        q = r["indicators"]["quote"][0]
        # 重複日（Yahoo 另附即時報價）：後者 O/H/L 為 null → 不可蓋掉完整的 K
        r["timestamp"].append(r["timestamp"][-2])             # 10/1 再來一筆
        for k, v in q.items():
            v.append(None)
        q["close"][-1] = 12345.0
        res = m.parse_twii_daily(j, tpe(2026, 10, 2, 10, 50))
        d1001 = [c for c in res if c["date"] == "2026-10-01"]
        self.assertEqual(len(d1001), 1)
        self.assertAlmostEqual(d1001[0]["close"], 48353.49, places=2)
        for c in res:
            for k in ("open", "high", "low", "close"):
                self.assertIsInstance(c[k], float)

    def test_unsorted_input_is_sorted(self):
        j = copy.deepcopy(self.chart)
        r = j["chart"]["result"][0]
        order = list(reversed(range(len(r["timestamp"]))))
        r["timestamp"] = [r["timestamp"][i] for i in order]
        for k, v in r["indicators"]["quote"][0].items():
            r["indicators"]["quote"][0][k] = [v[i] for i in order]
        res = m.parse_twii_daily(j, tpe(2026, 10, 2, 10, 50))
        dates = [c["date"] for c in res]
        self.assertEqual(dates, sorted(dates))
        self.assertEqual(dates[-1], "2026-10-01")

    def test_fewer_than_40_is_rejected(self):
        j = chart_with_filter(self.chart, lambda d: d.date() >= date(2026, 9, 1))
        with self.assertRaises(ValueError):
            m.parse_twii_daily(j, tpe(2026, 10, 2, 10, 50))

    def test_failure_keeps_old_and_records_error(self):
        old = m.parse_twii_daily(self.chart, tpe(2026, 10, 1, 15, 0))
        errors: list = []
        with mock.patch.object(m, "http_json", side_effect=OSError("來源無回應")):
            res = m.fetch_twii_daily(errors, old, tpe(2026, 10, 2, 15, 0))
        self.assertIs(res, old)
        self.assertEqual(len(errors), 1)
        self.assertIn("來源無回應", errors[0])

    def test_success_replaces_old(self):
        old = [{"date": "2026-01-02", "open": 1.0, "high": 1.0, "low": 1.0, "close": 1.0}]
        errors: list = []
        with mock.patch.object(m, "http_json", return_value=self.chart):
            res = m.fetch_twii_daily(errors, old, tpe(2026, 10, 2, 10, 50))
        assert res is not None
        self.assertIsNot(res, old)
        self.assertGreaterEqual(len(res), 40)
        self.assertEqual(errors, [])

    def test_does_not_regress_to_older_day_than_old(self):
        # task 6.4（審查 R3-low）：Yahoo 偶發回較舊的視窗（最後一根早於上次）→ 沿用舊值並記錯誤
        old = m.parse_twii_daily(self.chart, tpe(2026, 10, 2, 14, 0))
        self.assertEqual(old[-1]["date"], "2026-10-02")
        errors: list = []
        with mock.patch.object(m, "http_json", return_value=self.chart):
            res = m.fetch_twii_daily(errors, old, tpe(2026, 10, 2, 10, 50))
        self.assertIs(res, old, "新抓的最後一根（10/1）早於舊值（10/2）")
        self.assertEqual(len(errors), 1)
        self.assertIn("日K", errors[0])
        self.assertIn("2026-10-01", errors[0])

    def test_truncated_response_keeps_old_and_records_error(self):
        # task 6.4（審查 R3-M）：回應本體傳到一半斷線（http.client.IncompleteRead 不是 OSError）
        old = m.parse_twii_daily(self.chart, tpe(2026, 10, 1, 15, 0))
        errors: list = []
        with mock.patch.object(m, "http_json",
                               side_effect=http.client.IncompleteRead(b"abc", 100)):
            res = m.fetch_twii_daily(errors, old, tpe(2026, 10, 2, 15, 0))
        self.assertIs(res, old)
        self.assertEqual(len(errors), 1)
        self.assertIn("IncompleteRead", errors[0])


def margin_http(ms: str, al: str, day_all: str, calls: list | None = None):
    """依網址分派的假 http_json；calls 收集被請求的網址"""
    def fake(url: str):
        if calls is not None:
            calls.append(url)
        if "MI_MARGN" in url and "selectType=MS" in url:
            return load(ms)
        if "MI_MARGN" in url and "selectType=ALL" in url:
            return load(al)
        if "STOCK_DAY_ALL" in url:
            return load(day_all)
        raise AssertionError(f"非預期的網址：{url}")
    return fake


class MarginTest(unittest.TestCase):
    def test_short_margin_ratio_20260930(self):
        s = m.parse_margin_summary(load("margn_ms_20260930.json"))
        self.assertEqual(s["date"], "2026-09-30")
        self.assertEqual(s["margin_lots"], 9286318)
        self.assertEqual(s["short_lots"], 235296)
        self.assertEqual(s["margin_amount_k"], 622252453)
        self.assertAlmostEqual(s["short_lots"] / s["margin_lots"] * 100, 2.53, places=2)

    def test_stocks_rows_sum_to_summary(self):
        for d in ("20260930", "20261001"):
            s = m.parse_margin_summary(load(f"margn_ms_{d}.json"))
            day, stocks = m.parse_margin_stocks(load(f"margn_all_{d}.json"))
            self.assertEqual(day, s["date"])
            self.assertEqual(sum(x[1] for x in stocks), s["margin_lots"])
            self.assertEqual(sum(x[2] for x in stocks), s["short_lots"])

    def test_day_all_prices_date_and_skip_empty(self):
        day, prices = m.parse_day_all_prices(load("stock_day_all_20261001.json"))
        self.assertEqual(day, "2026-10-01")
        self.assertIn("2330", prices)
        self.assertIn("0050", prices)
        self.assertNotIn("00666R", prices)          # ClosingPrice 為空字串
        self.assertTrue(all(p > 0 for p in prices.values()))

    def test_day_all_with_mixed_dates_is_rejected(self):
        rows = load("stock_day_all_20261001.json")
        rows[0] = dict(rows[0], Date="1150930")
        with self.assertRaises(ValueError):
            m.parse_day_all_prices(rows)

    def test_compute_skips_unpriced_and_keeps_etf(self):
        s = m.parse_margin_summary(load("margn_ms_20261001.json"))
        _, stocks = m.parse_margin_stocks(load("margn_all_20261001.json"))
        _, prices = m.parse_day_all_prices(load("stock_day_all_20261001.json"))
        out = m.compute_margin(s, stocks, prices)
        self.assertEqual(
            sorted(out["unpriced"]),
            ["00666R", "00707R", "1589", "2323", "2601", "6655", "910322"],
        )
        codes = {x[0] for x in stocks}
        self.assertIn("0050", codes)                 # ETF 有融資 → 計入
        self.assertNotIn("0050", out["unpriced"])
        self.assertAlmostEqual(out["short_margin_ratio"], 2.49, places=2)
        # 獨立手算：Σ(融資餘額張 × 1000 × 收盤價) ÷ (融資金額仟元 × 1000) × 100
        self.assertAlmostEqual(out["maintenance_ratio"], 195.08, places=2)

    def test_compute_hand_checked_small_case(self):
        s = {"date": "2026-01-02", "margin_lots": 35, "short_lots": 7, "margin_amount_k": 1500}
        stocks = [("1111", 10, 1), ("2222", 20, 2), ("3333", 0, 0), ("4444", 5, 0)]
        out = m.compute_margin(s, stocks, {"1111": 100.0, "2222": 50.0, "3333": 9.0})
        # 10×1000×100 + 20×1000×50 = 2,000,000；融資金額 1,500 仟元＝1,500,000 元
        self.assertAlmostEqual(out["maintenance_ratio"], 133.33, places=2)
        self.assertAlmostEqual(out["short_margin_ratio"], 20.0, places=6)
        self.assertEqual(out["unpriced"], ["4444"])
        self.assertEqual(out["date"], "2026-01-02")

    def test_compute_rejects_inconsistent_rows(self):
        s = {"date": "2026-01-02", "margin_lots": 1000, "short_lots": 3, "margin_amount_k": 1500}
        with self.assertRaises(ValueError):
            m.compute_margin(s, [("1111", 10, 0)], {"1111": 100.0})

    def test_compute_rejects_zero_margin(self):
        s = {"date": "2026-01-02", "margin_lots": 0, "short_lots": 0, "margin_amount_k": 0}
        with self.assertRaises(ValueError):
            m.compute_margin(s, [], {})

    def test_fetch_same_day_prices_computes_and_requests_by_date(self):
        calls: list = []
        errors: list = []
        fake = margin_http("margn_ms_20261001.json", "margn_all_20261001.json",
                           "stock_day_all_20261001.json", calls)
        with mock.patch.object(m, "http_json", side_effect=fake):
            res = m.fetch_margin(errors, None)
        assert res is not None
        self.assertEqual(res["date"], "2026-10-01")
        self.assertEqual(res["margin_lots"], 9352816)
        self.assertEqual(res["short_lots"], 233322)
        self.assertAlmostEqual(res["short_margin_ratio"], 2.49, places=2)
        self.assertAlmostEqual(res["maintenance_ratio"], 195.08, places=2)
        self.assertEqual(errors, [])
        all_calls = [u for u in calls if "selectType=ALL" in u]
        self.assertEqual(len(all_calls), 1)
        self.assertIn("date=20261001", all_calls[0], "個股明細要用彙總的日期取，避免兩次請求跨過公布時點")

    def test_fetch_prices_newer_than_margin_keeps_old(self):
        # 15:00 那輪：價格已是 10/1、融資仍是 9/30 → 不得混用，沿用舊值
        old = {"date": "2026-09-29", "margin_lots": 1, "short_lots": 1,
               "short_margin_ratio": 1.0, "maintenance_ratio": 1.0, "unpriced": []}
        errors: list = []
        fake = margin_http("margn_ms_20260930.json", "margn_all_20260930.json",
                           "stock_day_all_20261001.json")
        with mock.patch.object(m, "http_json", side_effect=fake):
            res = m.fetch_margin(errors, old)
        assert res is not None
        self.assertIs(res, old)
        self.assertEqual(res["date"], "2026-09-29")
        self.assertEqual(errors, [], "晚間公布前屬正常情況，不算錯誤")

    def test_fetch_mismatch_without_old_returns_none(self):
        errors: list = []
        fake = margin_http("margn_ms_20260930.json", "margn_all_20260930.json",
                           "stock_day_all_20261001.json")
        with mock.patch.object(m, "http_json", side_effect=fake):
            self.assertIsNone(m.fetch_margin(errors, None))

    def test_fetch_same_date_as_old_skips_heavy_downloads(self):
        old = {"date": "2026-10-01", "margin_lots": 1, "short_lots": 1,
               "short_margin_ratio": 1.0, "maintenance_ratio": 1.0, "unpriced": []}
        calls: list = []
        errors: list = []
        fake = margin_http("margn_ms_20261001.json", "margn_all_20261001.json",
                           "stock_day_all_20261001.json", calls)
        with mock.patch.object(m, "http_json", side_effect=fake):
            res = m.fetch_margin(errors, old)
        self.assertIs(res, old)
        self.assertEqual(len(calls), 1)

    def test_fetch_failure_keeps_old_and_records_error(self):
        old = {"date": "2026-09-30", "margin_lots": 9286318, "short_lots": 235296,
               "short_margin_ratio": 2.53, "maintenance_ratio": 195.7, "unpriced": []}
        errors: list = []
        with mock.patch.object(m, "http_json", side_effect=OSError("來源無回應")):
            res = m.fetch_margin(errors, old)
        self.assertIs(res, old)
        self.assertEqual(len(errors), 1)
        self.assertIn("來源無回應", errors[0])

    def test_fetch_no_data_stat_keeps_old_and_records_error(self):
        old = {"date": "2026-09-30"}
        errors: list = []
        with mock.patch.object(m, "http_json", return_value={"stat": "很抱歉，沒有符合條件的資料"}):
            res = m.fetch_margin(errors, old)
        self.assertIs(res, old)
        self.assertEqual(len(errors), 1)

    def test_fetch_ms_and_all_date_mismatch_is_error(self):
        old = {"date": "2026-09-29"}
        errors: list = []
        fake = margin_http("margn_ms_20261001.json", "margn_all_20260930.json",
                           "stock_day_all_20261001.json")
        with mock.patch.object(m, "http_json", side_effect=fake):
            res = m.fetch_margin(errors, old)
        self.assertIs(res, old)
        self.assertEqual(len(errors), 1)


class MalformedOldTest(unittest.TestCase):
    """舊輸出檔的新鍵形狀不對（降版、手動編輯）時，不可讓整個 main() 崩潰"""

    BAD_OLD = (["x"], "字串", 5, [])

    def test_intraday_malformed_old(self):
        chart = load("yahoo_twii_5m_5d.json")
        for bad in self.BAD_OLD:
            with self.subTest(old=bad):
                errors: list = []
                with mock.patch.object(m, "http_json", return_value=chart):
                    res = m.fetch_twii_intraday(errors, bad, tpe(2026, 10, 1, 15, 0))
                assert res is not None
                self.assertEqual(res["date"], "2026-10-01")
                with mock.patch.object(m, "http_json", side_effect=OSError("x")):
                    self.assertIsNone(m.fetch_twii_intraday(errors, bad, tpe(2026, 10, 1, 15, 0)))

    def test_daily_malformed_old(self):
        chart = load("yahoo_twii_1d_3mo.json")
        for bad in ({"a": 1}, "字串", 5):
            with self.subTest(old=bad):
                errors: list = []
                with mock.patch.object(m, "http_json", return_value=chart):
                    res = m.fetch_twii_daily(errors, bad, tpe(2026, 10, 2, 10, 50))
                assert res is not None
                self.assertGreaterEqual(len(res), 40)
                with mock.patch.object(m, "http_json", side_effect=OSError("x")):
                    self.assertIsNone(m.fetch_twii_daily(errors, bad, tpe(2026, 10, 2, 10, 50)))

    def test_margin_malformed_old(self):
        fake = margin_http("margn_ms_20261001.json", "margn_all_20261001.json",
                           "stock_day_all_20261001.json")
        for bad in (["x"], "字串", 5):
            with self.subTest(old=bad):
                errors: list = []
                with mock.patch.object(m, "http_json", side_effect=fake):
                    res = m.fetch_margin(errors, bad)
                assert res is not None
                self.assertEqual(res["date"], "2026-10-01")
                with mock.patch.object(m, "http_json", side_effect=OSError("x")):
                    self.assertIsNone(m.fetch_margin(errors, bad))


class MainIntegrationTest(unittest.TestCase):
    """main() 層級：新鍵不得改變既有鍵 fetched／errors 的語意（spec：新鍵 MUST NOT 改變既有鍵）"""

    OLD_FETCHED = "2026-09-01 10:00"

    def run_main(self, *, fail_daily: bool = False, daily_exc: BaseException | None = None,
                 margin_exc: BaseException | None = None, holidays_ok: bool = False,
                 patches: tuple = ()) -> dict:
        """原有來源全部失敗；新鍵三來源成功（fail_daily＝日 K 來源另外失敗）。回傳輸出的 payload。
        daily_exc／margin_exc：該來源改丟這個例外；holidays_ok：既有的休市日曆來源成功（看既有鍵有沒有更新）；
        patches：另外套用的 mock.patch（例如讓某個 fetch 函式本身丟例外）"""
        def fake(url: str, *a, **k):
            if url == m.URL_TWII_INTRADAY:
                return load("yahoo_twii_5m_5d.json")
            if url == m.URL_TWII_DAILY and daily_exc is not None:
                raise daily_exc
            if url == m.URL_TWII_DAILY and not fail_daily:
                return load("yahoo_twii_1d_3mo.json")
            if "MI_MARGN" in url and margin_exc is not None:
                raise margin_exc
            if url == m.URL_HOLIDAY and holidays_ok:
                return load("holiday_schedule_20261002.json")
            if "MI_MARGN" in url and "selectType=MS" in url:
                return load("margn_ms_20261001.json")
            if "MI_MARGN" in url and "selectType=ALL" in url:
                return load("margn_all_20261001.json")
            if url == m.URL_DAY_ALL:
                return load("stock_day_all_20261001.json")
            raise OSError("模擬來源失敗")

        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            (tmp / "tw_events.json").write_text(json.dumps(
                {"fetched": self.OLD_FETCHED, "updated": self.OLD_FETCHED}), encoding="utf-8")
            with (mock.patch.object(m, "__file__", str(tmp / "update_tw_events.py")),
                  mock.patch.object(m, "http_json", side_effect=fake),
                  mock.patch.object(m, "http_bytes", side_effect=OSError("模擬來源失敗")),
                  mock.patch.object(m, "wait_for_network", return_value=True),
                  mock.patch.object(m, "lively_wallpaper_dirs", return_value=[]),
                  mock.patch.object(m, "log"),
                  mock.patch.object(sys, "argv", ["update_tw_events.py"]),
                  contextlib.ExitStack() as stack):
                for p in patches:
                    stack.enter_context(p)
                rc = m.main(now=tpe(2026, 10, 1, 15, 0))
            self.assertEqual(rc, 0)
            return json.loads((tmp / "tw_events.json").read_text(encoding="utf-8"))

    def test_truncated_responses_in_new_keys_never_stop_the_round(self):
        # task 6.4（審查 R3-M）：新鍵來源的回應傳到一半斷線（IncompleteRead）或時間戳離譜（OverflowError），
        # 不得讓 main() 整輪不寫檔：既有鍵照常更新，新鍵沿用舊值（這裡沒有舊值＝省略），錯誤記進 wallpaper_errors
        out = self.run_main(daily_exc=http.client.IncompleteRead(b"abc", 100),
                            margin_exc=OverflowError("timestamp out of range"), holidays_ok=True)
        self.assertIn("holidays", out, "既有鍵（休市日曆）照常更新")
        self.assertNotEqual(out["fetched"], self.OLD_FETCHED, "有既有來源成功，fetched 照常刷新")
        self.assertIn("twii_intraday", out, "其他新鍵照常更新")
        self.assertNotIn("twii_daily", out)
        self.assertNotIn("margin", out)
        self.assertEqual(len(out["wallpaper_errors"]), 2, out["wallpaper_errors"])
        self.assertTrue(any("IncompleteRead" in e for e in out["wallpaper_errors"]))
        self.assertTrue(any("timestamp out of range" in e for e in out["wallpaper_errors"]))

    def test_unexpected_exception_in_a_new_key_never_stops_the_round(self):
        # 防線第二層：新鍵的抓取函式即使丟出白名單以外的例外（程式錯誤等），也只記錯誤、不拖垮整輪
        boom = mock.patch.object(m, "fetch_margin", side_effect=RuntimeError("意外的程式錯誤"))
        out = self.run_main(holidays_ok=True, patches=(boom,))
        self.assertIn("holidays", out)
        self.assertIn("twii_daily", out)
        self.assertNotIn("margin", out)
        self.assertTrue(any("意外的程式錯誤" in e for e in out["wallpaper_errors"]),
                        out["wallpaper_errors"])

    def test_new_keys_do_not_refresh_fetched(self):
        out = self.run_main()
        for k in ("twii_intraday", "twii_daily", "margin"):
            self.assertIn(k, out, "新鍵有抓到")
        self.assertTrue(out["errors"], "原有來源確實都失敗了")
        self.assertEqual(out["fetched"], self.OLD_FETCHED,
                         "只有新鍵抓到新資料時，fetched 必須維持舊值（Lively 的「已 N 天未更新」靠它）")
        self.assertEqual(out["wallpaper_errors"], [])

    def test_new_key_failure_goes_to_wallpaper_errors_not_errors(self):
        ok = self.run_main()
        bad = self.run_main(fail_daily=True)
        self.assertNotIn("twii_daily", bad, "沒有舊值又失敗＝省略該鍵")
        self.assertEqual(len(bad["wallpaper_errors"]), 1)
        self.assertIn("日K", bad["wallpaper_errors"][0])
        self.assertEqual(bad["errors"], ok["errors"], "新鍵失敗不得動到既有的 errors")
        self.assertFalse(any("日K" in e for e in bad["errors"]))


class HolidaysTest(unittest.TestCase):
    """fetch_holidays：holidaySchedule 混有「開始交易日／最後交易日」說明列，那是交易日、不可當休市"""

    FIXTURE = "holiday_schedule_20261002.json"

    def run_fetch(self, raw) -> list:
        errors: list = []
        with mock.patch.object(m, "http_json", return_value=raw):
            out = m.fetch_holidays(errors)
        self.assertEqual(errors, [])
        assert out is not None
        return out

    def test_trading_day_notice_rows_excluded(self):
        out = self.run_fetch(load(self.FIXTURE))
        for d in ("2026-01-02", "2026-02-11", "2026-02-23"):
            self.assertNotIn(d, out, f"{d} 是交易日（開始／最後交易日說明列），不得列為休市")

    def test_settlement_only_days_kept(self):
        out = self.run_fetch(load(self.FIXTURE))
        for d in ("2026-02-12", "2026-02-13"):
            self.assertIn(d, out, f"{d}「市場無交易，僅辦理結算交割作業」是真的不交易，必須保留")

    def test_regular_holidays_kept(self):
        out = self.run_fetch(load(self.FIXTURE))
        for d in ("2026-01-01", "2026-02-16", "2026-02-20", "2026-04-03",
                  "2026-10-09", "2026-12-25"):
            self.assertIn(d, out)
        self.assertEqual(out, sorted(set(out)), "輸出維持排序、去重")

    # ── 歷年真實資料（TWSE 網站 rwd 端點，2021–2025；2026 用 openapi 那份）──
    # 期望的「交易日說明列」日期＝逐列閱讀 Name／Description 語意後人工列出，不由實作推導。
    # 2021／2022 年結算交割日的 Name 也寫「農曆春節前最後交易日」，只有 Description 寫「市場無交易」，
    # 故以下清單刻意不含 2021-02-08/09、2022-01-27/28；2023-01-17 是交易日，其說明第二句提到別天無交易。
    TRADING_DAYS: ClassVar[dict[int, list[str]]] = {
        2021: ["2021-01-04", "2021-02-05", "2021-02-17"],
        2022: ["2022-01-03", "2022-01-26", "2022-02-07"],
        2023: ["2023-01-03", "2023-01-17", "2023-01-30"],
        2024: ["2024-01-02", "2024-02-05", "2024-02-15"],
        2025: ["2025-01-02", "2025-01-22", "2025-02-03"],
        2026: ["2026-01-02", "2026-02-11", "2026-02-23"],
    }

    @staticmethod
    def rwd_rows(year: int) -> list[dict]:
        """rwd 回應（{data: [[日期, 名稱, 說明], ...]}，日期為西元 ISO）→ openapi 形狀（Date 民國 7 碼）。
        只在測試層轉形狀，正式程式仍只抓 openapi"""
        rows = []
        for d, name, desc in load(f"holiday_schedule_rwd_{year}_20261002.json")["data"]:
            y, mo, dd = d.split("-")
            rows.append({"Name": name, "Date": f"{int(y) - 1911:03d}{mo}{dd}", "Description": desc})
        return rows

    def raw_for(self, year: int) -> list[dict]:
        return load(self.FIXTURE) if year == 2026 else self.rwd_rows(year)

    def test_settlement_days_named_as_last_trading_day_are_kept(self):
        """2021／2022 的真實寫法：Name＝農曆春節前最後交易日，Description 第一句寫市場無交易"""
        for year, days in ((2021, ["2021-02-08", "2021-02-09"]),
                           (2022, ["2022-01-27", "2022-01-28"])):
            out = self.run_fetch(self.raw_for(year))
            for d in days:
                self.assertIn(d, out, f"{d} 市場無交易、僅辦理結算交割，是真休市")

    def test_trading_day_whose_description_mentions_other_days_no_trading(self):
        """2023-01-17 是交易日；說明第二句提到 1/18、1/19 無交易，不可因此被當成休市"""
        out = self.run_fetch(self.raw_for(2023))
        self.assertNotIn("2023-01-17", out)
        for d in ("2023-01-18", "2023-01-19"):
            self.assertIn(d, out)

    def test_every_row_2021_to_2026_classified_correctly(self):
        """每年每一列：輸出＝資料中所有日期扣掉人工列出的交易日說明列"""
        for year, trading in self.TRADING_DAYS.items():
            raw = self.raw_for(year)
            all_dates = {
                f"{int(r['Date'][:3]) + 1911}-{r['Date'][3:5]}-{r['Date'][5:7]}" for r in raw}
            for d in trading:
                self.assertIn(d, all_dates, f"{year} 資料裡應有交易日說明列 {d}（fixture 或清單有誤）")
            self.assertEqual(self.run_fetch(raw), sorted(all_dates - set(trading)), f"{year}")


class OutputShapeTest(unittest.TestCase):
    def test_new_keys_are_json_serializable(self):
        chart = load("yahoo_twii_5m_5d.json")
        daily = load("yahoo_twii_1d_3mo.json")
        payload = {
            "twii_intraday": m.parse_twii_intraday(chart, tpe(2026, 10, 1, 15, 0)),
            "twii_daily": m.parse_twii_daily(daily, tpe(2026, 10, 2, 10, 50)),
        }
        text = json.dumps(payload, ensure_ascii=False)
        self.assertEqual(json.loads(text), payload)


if __name__ == "__main__":
    unittest.main()
