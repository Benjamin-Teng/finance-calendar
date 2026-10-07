"""Python oracle（tests/fetch_oracle/）的測試：錄製樣本可重現、回放不碰真實網路、scenario 覆寫機制、邊界情境樣本。

執行：uv run --no-project python -m unittest discover -s tests -v
"""

import contextlib
import io
import json
import re
import shutil
import socket
import sys
import tempfile
import unittest
import urllib.request
import warnings
from datetime import datetime
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

# 回放丟出的 HTTPError 帶 BytesIO 本體，腳本不會 close 它；垃圾回收時的 ResourceWarning 與本測試無關
warnings.filterwarnings("ignore", message="Implicitly cleaning up", category=ResourceWarning)


def setUpModule():
    # unittest 執行器會在跑測試前把警告過濾重設為 default，模組載入時的過濾會被蓋掉；在這裡重新加上
    warnings.filterwarnings("ignore", message="Implicitly cleaning up", category=ResourceWarning)

TESTS = Path(__file__).resolve().parent
sys.path.insert(0, str(TESTS / "fetch_oracle"))

import expect  # ty: ignore[unresolved-import]
import harness  # ty: ignore[unresolved-import]

FETCH_FIXTURES = harness.REPO_ROOT / "host" / "tests" / "fixtures" / "fetch"
RECORDED = sorted(p for p in FETCH_FIXTURES.glob("recorded-*") if p.is_dir())
URL_HOLIDAY = "https://openapi.twse.com.tw/v1/holidaySchedule/holidaySchedule"


@contextlib.contextmanager
def no_network():
    """回放期間任何真實網路存取都記下並拒絕；呼叫端以 calls 是否為空判定"""
    calls: list = []

    def boom(*args, **kwargs):
        calls.append(args)
        raise OSError("測試禁止真實網路")

    with (
        mock.patch.object(socket.socket, "connect", boom),
        mock.patch.object(socket, "create_connection", boom),
        mock.patch.object(socket, "getaddrinfo", boom),
        mock.patch.object(urllib.request, "urlopen", boom),
    ):
        yield calls


def run_offline(d: Path, for_expected: bool = False) -> dict:
    with no_network() as calls:
        result = expect.run_scenario(d, for_expected=for_expected)
    assert not calls, f"回放期間出現真實網路請求：{calls}"
    return result.output


def write_scenario(d: Path, scenario: dict, old: dict | None = None) -> Path:
    d.mkdir(parents=True, exist_ok=True)
    (d / "scenario.json").write_text(json.dumps(scenario), encoding="utf-8")
    if old is not None:
        (d / "old.json").write_text(json.dumps(old), encoding="utf-8")
    return d


class RecordedSampleTest(unittest.TestCase):
    def test_sample_exists(self):
        self.assertTrue(RECORDED, "host/tests/fixtures/fetch/ 下沒有 recorded-* 樣本")

    def test_replay_is_reproducible_and_matches_committed_files(self):
        for d in RECORDED:
            with self.subTest(sample=d.name):
                first, second = run_offline(d), run_offline(d)
                self.assertEqual(first, second)
                expected_text = (d / "expected.json").read_bytes().decode("utf-8")
                self.assertEqual(harness.render(first), expected_text)
                # 凍結時間套到每一處後，回放輸出與錄製當下的實際輸出逐字相同（含 updated／fetched）
                live = json.loads((d / "live_output.json").read_text(encoding="utf-8"))
                self.assertEqual(first, live)

    def test_frozen_time_reaches_both_uses(self):
        for d in RECORDED:
            with self.subTest(sample=d.name):
                manifest = json.loads((d / "manifest.json").read_text(encoding="utf-8"))
                out = run_offline(d)
                tpe, local = manifest["recorded_at_tpe"], manifest["recorded_at_local"]
                self.assertEqual(out["window"]["start"], tpe[:10])                 # B-DATE-2
                self.assertEqual(out["updated"], local[:10] + " " + local[11:16])  # B-DATE-3

    def test_every_recorded_request_was_replayed_or_known(self):
        # 錄到的每個請求都該被回放用到：回放輸出沒有「查無對應」的錯誤
        for d in RECORDED:
            with self.subTest(sample=d.name):
                self.assertFalse([e for e in run_offline(d)["errors"] if "查無對應" in e])


class SourceTimeSourcesTest(unittest.TestCase):
    def test_script_has_no_unfrozen_time_source(self):
        """harness 只凍結 datetime.now(...)；腳本若新增其他取時間的寫法，oracle 就得跟著處理"""
        lines = harness.SCRIPT.read_text(encoding="utf-8").splitlines()
        src = "\n".join(line.split("#")[0] for line in lines)   # 註解不算
        bad = re.findall(r"\b(?:date\.today|datetime\.today|datetime\.utcnow|time\.time|time\.monotonic"
                         r"|time\.localtime|time\.gmtime|time\.strftime)\(", src)
        self.assertEqual(bad, [])
        # 已知且已凍結的 datetime.now 呼叫點（B-DATE-2、B-DATE-3）：共 7 處
        self.assertEqual(len(re.findall(r"datetime\.now\(", src)), 7)


class ScenarioTest(unittest.TestCase):
    def setUp(self):
        self.assertTrue(RECORDED)
        self.base = RECORDED[-1].resolve()
        self.tmp = Path(tempfile.mkdtemp(prefix="fc-oracle-test-"))
        self.addCleanup(lambda: shutil.rmtree(self.tmp, ignore_errors=True))

    def test_base_without_changes_equals_base(self):
        d = write_scenario(self.tmp / "same", {"base": str(self.base)})
        self.assertEqual(run_offline(d), run_offline(self.base))

    def test_override_one_request_to_500(self):
        base_out = run_offline(self.base)
        self.assertIn("holidays", base_out)
        d = write_scenario(self.tmp / "h500", {
            "base": str(self.base),
            "overrides": [{"method": "GET", "url": URL_HOLIDAY, "form": None,
                           "status": 500, "reason": "Internal Server Error"}],
        })
        out = run_offline(d)
        self.assertNotIn("holidays", out)   # 全新（無舊檔）又失敗＝省略該鍵
        self.assertIn("休市日曆來源失敗：HTTP Error 500: Internal Server Error", out["errors"])
        # 其餘來源不受影響；base 本身也沒被改到
        self.assertEqual(out["counts"], base_out["counts"])
        self.assertEqual(run_offline(self.base), base_out)

    def test_override_connection_error_becomes_urlerror(self):
        d = write_scenario(self.tmp / "conn", {
            "base": str(self.base),
            "overrides": [{"url": URL_HOLIDAY, "error": "TimeoutError: timed out"}],
        })
        self.assertIn("休市日曆來源失敗：timed out", run_offline(d)["errors"])

    def test_old_json_is_used_as_previous_output(self):
        d = write_scenario(self.tmp / "old", {
            "base": str(self.base),
            "overrides": [{"url": URL_HOLIDAY, "status": 500}],
        }, old={"holidays": ["2026-01-01"]})
        out = run_offline(d)
        self.assertEqual(out["holidays"], ["2026-01-01"])   # 失敗沿用舊檔（B-FLOW 的 fallback）

    def test_time_override_splits_tpe_and_local(self):
        d = write_scenario(self.tmp / "t", {
            "base": str(self.base),
            "now_tpe": "2026-10-05T23:30:00+08:00",
            "now_local": "2026-10-05T16:30:00",
        })
        out = run_offline(d)
        self.assertEqual(out["window"]["start"], "2026-10-05")   # 台北日期
        self.assertEqual(out["updated"], "2026-10-05 16:30")     # 本機牆上時間
        d2 = write_scenario(self.tmp / "t2", {"base": str(self.base), "now_tpe": "2026-10-06T00:10:00+08:00"})
        out2 = run_offline(d2)
        self.assertEqual(out2["window"]["start"], "2026-10-06")
        self.assertEqual(out2["updated"], "2026-10-06 00:10")    # 只給 now_tpe＝本機視同台北

    def test_unrecorded_request_raises_urlerror_for_every_source(self):
        d = self.tmp / "empty"
        d.mkdir()
        (d / "manifest.json").write_text(json.dumps({
            "recorded_at_tpe": "2026-10-05T10:00:00+08:00",
            "recorded_at_local": "2026-10-05T10:00:00",
            "requests": [],
        }), encoding="utf-8")
        out = run_offline(d)
        self.assertTrue(out["errors"])
        self.assertTrue(all("查無對應" in e or e.startswith("法說會") for e in out["errors"]), out["errors"])
        self.assertEqual(out["counts"]["quotes"], 0)

    def test_expect_cli_writes_expected_json_lf(self):
        d = write_scenario(self.tmp / "cli", {"base": str(self.base)})
        with no_network() as calls, contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(expect.main([str(d)]), 0)
        self.assertEqual(calls, [])
        raw = (d / "expected.json").read_bytes()
        self.assertTrue(raw.endswith(b"}\n"))
        self.assertNotIn(b"\r", raw)
        self.assertEqual(json.loads(raw), run_offline(self.base))


class IsolationTest(unittest.TestCase):
    def test_leak_into_watched_dir_is_detected(self):
        with tempfile.TemporaryDirectory() as fake_root:
            def factory(orig):
                def leaky(url, form=None):
                    (Path(fake_root) / "tw_events.json").write_text("{}", encoding="utf-8")
                    raise OSError("x")
                return leaky

            with (
                mock.patch.object(harness, "REPO_ROOT", Path(fake_root)),
                self.assertRaises(harness.IsolationLeak),
            ):
                harness.run_isolated(harness.parse_frozen("2026-10-05T10:00:00+08:00"), factory)

    def test_run_leaves_no_outputs_in_repo_root_and_temp_is_removed(self):
        before = harness.snapshot_outputs([harness.REPO_ROOT])
        tmp_before = set(Path(tempfile.gettempdir()).glob("fc-oracle-*"))
        if RECORDED:
            run_offline(RECORDED[-1])
        self.assertEqual(harness.snapshot_outputs([harness.REPO_ROOT]), before)
        self.assertEqual(set(Path(tempfile.gettempdir()).glob("fc-oracle-*")), tmp_before)


class ScenarioResolutionTest(unittest.TestCase):
    """base 為相對路徑、override 的回應檔路徑解析、expected_from（task 1.1 審查 M2、task 1.3）"""

    def setUp(self):
        self.base = RECORDED[-1].resolve()
        self.tmp = Path(tempfile.mkdtemp(prefix="fc-oracle-test-"))
        self.addCleanup(lambda: shutil.rmtree(self.tmp, ignore_errors=True))

    def test_relative_base_and_response_files_resolve_from_the_declaring_directory(self):
        # top 以相對路徑指向 mid、mid 再指向絕對的錄製樣本；兩層各自宣告回應檔，路徑都相對於宣告它的那個目錄
        mid = write_scenario(self.tmp / "mid", {
            "base": str(self.base),
            "overrides": [{"url": URL_HOLIDAY, "file": "h.json"}],
        })
        (mid / "h.json").write_text(json.dumps([{"Name": "測試休市", "Date": "1150303"}]), encoding="utf-8")
        top = write_scenario(self.tmp / "top", {
            "base": "../mid",
            "overrides": [{"url": "https://openapi.twse.com.tw/v1/opendata/t187ap41_L", "file": "m.json"}],
        })
        (top / "m.json").write_text(json.dumps([{
            "開會日期": "1151010", "股東常(臨時)會": "臨時會", "是否改選董監": "否",
            "公司代號": "9999", "公司名稱": "測試"}]), encoding="utf-8")
        out = run_offline(top)
        self.assertEqual(out["holidays"], ["2026-03-03"])          # 檔案在 mid，由 mid 的目錄解析
        meetings = [e for e in out["events"] if e["type"] == "meeting"]
        self.assertEqual([(e["date"], e["code"], e["note"]) for e in meetings],
                         [("2026-10-10", "9999", "股東臨時會")])    # 檔案在 top，由 top 的目錄解析
        self.assertEqual(run_offline(mid)["holidays"], ["2026-03-03"])   # 疊上 top 不影響 mid 自己

    def test_expected_from_overrides_apply_only_to_expected(self):
        d = write_scenario(self.tmp / "ef", {
            "base": str(self.base),
            "overrides": [{"url": URL_HOLIDAY, "status": 500}],
            "expected_from": {"overrides": [{"url": URL_HOLIDAY, "file": "ok.json"}]},
        })
        (d / "ok.json").write_text(json.dumps([{"Name": "測試休市", "Date": "1150303"}]), encoding="utf-8")
        self.assertNotIn("holidays", run_offline(d))                         # 原始輸入：來源失敗
        self.assertEqual(run_offline(d, for_expected=True)["holidays"], ["2026-03-03"])
        with no_network(), contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(expect.main([str(d)]), 0)                        # CLI 寫的是 expected_from 的結果
        self.assertEqual(json.loads((d / "expected.json").read_bytes())["holidays"], ["2026-03-03"])

    def test_expected_from_old_selects_another_file_or_none(self):
        old = {"holidays": ["2026-01-01"]}
        spec = {"base": str(self.base), "overrides": [{"url": URL_HOLIDAY, "status": 500}]}
        d = write_scenario(self.tmp / "o1", dict(spec, expected_from={"old": "other.json"}), old=old)
        (d / "other.json").write_text(json.dumps({"holidays": ["2026-02-02"]}), encoding="utf-8")
        self.assertEqual(run_offline(d)["holidays"], ["2026-01-01"])                    # 原始輸入用 old.json
        self.assertEqual(run_offline(d, for_expected=True)["holidays"], ["2026-02-02"])
        d2 = write_scenario(self.tmp / "o2", dict(spec, expected_from={"old": None}), old=old)
        self.assertEqual(run_offline(d2)["holidays"], ["2026-01-01"])
        self.assertNotIn("holidays", run_offline(d2, for_expected=True))                # null＝不放舊檔
        d3 = write_scenario(self.tmp / "o3", dict(spec, expected_from={}), old=old)     # 省略 old＝沿用 old.json
        self.assertEqual(run_offline(d3, for_expected=True)["holidays"], ["2026-01-01"])

    def test_expected_from_rejects_unknown_keys(self):
        d = write_scenario(self.tmp / "bad", {"base": str(self.base), "expected_from": {"oldd": None}})
        with self.assertRaises(ValueError):
            expect.load_scenario(d, for_expected=True)
        expect.load_scenario(d)   # 原始輸入的載入不看 expected_from


# ───────────── task 1.3 邊界情境樣本：每個情境一個證據斷言 ─────────────
HTTP500 = "HTTP Error 500: Internal Server Error"
QUOTE_SYMBOLS = [("USD/TWD", "USDTWD=X"), ("US 10Y", "^TNX"), ("WTI 原油", "CL=F"), ("加權指數", "^TWII"),
                 ("台積電", "2330.TW"), ("日經225", "^N225"), ("KOSPI", "^KS11"), ("歐股50", "^STOXX50E"),
                 ("道瓊", "^DJI"), ("S&P 500", "^GSPC"), ("NASDAQ", "^IXIC"), ("費半", "^SOX")]
SCENARIOS = sorted(p.name for p in FETCH_FIXTURES.iterdir() if p.is_dir() and not p.name.startswith("recorded-"))


def prefixes(errors):
    return [e.split("：", 1)[0] for e in errors]


def events_of(out, *types):
    return [(e["date"], e["type"], e["code"], e["note"]) for e in out["events"] if e["type"] in types]


def quote_of(out, name):
    return next(q for q in out["quotes"] if q["name"] == name)


def by_code(out, market=None):
    return {p["code"]: p for p in out["punish"] if market is None or p["market"] == market}


def rows(out, codes_dates):
    return {code: (p["start"], p["end"], p["times"]) for code, p in by_code(out).items() if code in codes_dates}


def check_all_sources_fail_with_old(t, c):
    o, old = c.out, c.old
    t.assertEqual((o["fetched"], o["updated"]), ("2026-10-04 06:00", "2026-10-05 01:12"))
    for k in ("macro", "events", "punish", "quotes", "top100", "holidays", "twii_intraday", "twii_daily", "margin", "counts"):
        t.assertEqual(o[k], old[k], k)
    t.assertEqual(o["top100_date"], "2026-10-04")
    t.assertEqual(len(o["quotes"]), 13)
    want = (["總經來源失敗（本週檔）", "除權息來源失敗", "股東會來源失敗", "市值前百大來源失敗", "法說會來源（MOPS）失敗",
             "法說會備援來源（重大訊息）失敗", "處置股來源失敗（上市）", "處置股來源失敗（上櫃）"]
            + [f"行情來源失敗（{n}／{s}）" for n, s in QUOTE_SYMBOLS]
            + ["行情來源失敗（SOFR 3M／NY Fed）", "休市日曆來源失敗"])
    t.assertEqual(prefixes(o["errors"]), want)
    t.assertEqual(prefixes(o["wallpaper_errors"]), ["加權盤中走勢來源失敗", "加權日K來源失敗", "券資比／融資維持率來源失敗"])


def check_all_sources_fail_no_old(t, c):
    o = c.out
    for k in ("macro", "events", "punish", "quotes", "top100"):
        t.assertEqual(o[k], [], k)
    t.assertIsNone(o["top100_date"])
    t.assertEqual(o["counts"], dict.fromkeys(("dividend", "meeting", "conference", "earnings", "macro", "quotes", "punish"), 0))
    for k in ("holidays", "twii_intraday", "twii_daily", "margin"):
        t.assertNotIn(k, o)
    t.assertEqual(o["fetched"], o["updated"])
    t.assertEqual(len(o["errors"]), 22)
    t.assertIn("法說會：無市值前百大名單可篩，本輪改用備援來源", o["errors"])
    t.assertEqual(len(o["wallpaper_errors"]), 3)


def check_wallpaper_only_success(t, c):
    o, rec = c.out, c.rec
    t.assertEqual(c.old, {"updated": "2026-09-01 10:00", "fetched": "2026-09-01 10:00"})   # 舊檔沒有任何桌布鍵
    t.assertEqual((o["fetched"], o["updated"]), ("2026-09-01 10:00", "2026-10-05 01:12"))   # 桌布鍵不刷新 fetched
    for k in ("twii_intraday", "twii_daily", "margin"):
        t.assertEqual(o[k], rec[k], k)    # 值是本輪新抓的（與錄製樣本同一份回應），不是沿用
    t.assertEqual(o["wallpaper_errors"], [])
    t.assertEqual(len(o["errors"]), 22)
    t.assertFalse([e for e in o["errors"] if any(w in e for w in ("盤中走勢", "日K", "券資比"))])
    for k in ("macro", "events", "punish", "quotes", "top100"):
        t.assertEqual(o[k], [], k)
    t.assertIsNone(o["top100_date"])
    t.assertNotIn("holidays", o)


def check_punish_tpex_fail(t, c):
    o, old = c.out, c.old
    t.assertEqual(o["errors"], ["處置股來源失敗（上櫃）：" + HTTP500])
    tpex = [p for p in o["punish"] if p["market"] == "上櫃"]
    t.assertEqual(tpex, [p for p in old["punish"] if p["market"] == "上櫃"])
    t.assertEqual(sorted(p["code"] for p in tpex), ["3293", "6488"])
    t.assertEqual([p for p in o["punish"] if p["market"] == "上市"], [p for p in c.rec["punish"] if p["market"] == "上市"])
    t.assertNotIn("2002", by_code(o))
    t.assertEqual(o["counts"]["punish"], 10)
    t.assertEqual([p["code"] for p in o["punish"]], sorted(p["code"] for p in o["punish"]))


def check_punish_tpex_blank_template(t, c):
    o = c.out
    t.assertEqual(o["errors"], [])
    t.assertEqual(o["punish"], [p for p in c.rec["punish"] if p["market"] == "上市"])
    t.assertTrue({"3293", "6488", "2002"}.isdisjoint(by_code(o)))
    t.assertTrue(any(p["market"] == "上櫃" for p in c.old["punish"]))   # 舊檔確實有上櫃列可被（錯誤地）沿用


def check_punish_second_disposition(t, c):
    o = c.out
    t.assertEqual(o["errors"], [])
    got = by_code(o)
    t.assertEqual((got["2455"]["start"], got["2455"]["end"], got["2455"]["times"]), ("2026-10-06", "2026-10-16", 2))
    t.assertEqual((got["3016"]["start"], got["3016"]["end"], got["3016"]["times"]), ("2026-10-07", "2026-10-20", 2))
    t.assertEqual((got["3167"]["start"], got["3167"]["end"], got["3167"]["times"]), ("2026-10-05", "2026-10-12", 2))
    # end 相同時 start 較晚者勝，且與來源列順序無關（3167 較晚的在前、3055 較晚的在後）：擋「只比 end、平手前者勝／後者勝」兩種寫法
    t.assertEqual((got["3055"]["start"], got["3055"]["end"], got["3055"]["times"]), ("2026-10-05", "2026-10-12", 3))
    t.assertEqual((got["2221"]["market"], got["2221"]["start"], got["2221"]["end"], got["2221"]["times"]),
                  ("上市", "2026-10-13", "2026-10-19", 3))
    t.assertEqual((got["8084"]["market"], got["8084"]["end"]), ("上櫃", "2026-10-08"))
    t.assertEqual(len(o["punish"]), len(got))   # 每個代號只出現一次
    t.assertEqual([p["code"] for p in o["punish"]], sorted(got))


def check_punish_period_formats(t, c):
    o = c.out
    t.assertEqual(o["errors"], [])
    twse = by_code(o, "上市")
    t.assertEqual(sorted(twse), ["0050", "006208", "1101", "1102", "1216", "1301", "1303", "1326", "2891B"])
    for code in ("1101", "1102", "1216", "1301", "1303", "1326"):
        t.assertEqual((twse[code]["start"], twse[code]["end"]), ("2026-10-05", "2026-10-12"), code)
    for code in ("0050", "006208", "2891B"):
        t.assertEqual((twse[code]["start"], twse[code]["end"]), ("2026-10-06", "2026-10-13"), code)
    t.assertEqual({k: v["times"] for k, v in twse.items() if k in ("1101", "1102", "1216", "1301", "1303")},
                  {"1101": 4, "1102": 3, "1216": 1, "1301": 1, "1303": 2})
    tpex = by_code(o, "上櫃")
    t.assertEqual(sorted(tpex), ["3293", "4128", "6488"])
    t.assertEqual({k: (v["start"], v["end"], v["times"]) for k, v in tpex.items()},
                  {"3293": ("2026-10-05", "2026-10-12", 2), "4128": ("2026-10-05", "2026-10-12", 1),
                   "6488": ("2026-10-05", "2026-10-12", 1)})


def check_conference_range_date(t, c):
    o = c.out
    t.assertEqual(o["errors"], [])
    t.assertEqual(events_of(o, "earnings"), [("2026-10-06", "earnings", "2330", "Q3 財報（線上）"),
                                             ("2026-10-07", "earnings", "2454", "Q2 財報")])
    t.assertEqual(events_of(o, "conference"), [("2026-10-09", "conference", "2308", "法說會"),
                                               ("2026-10-14", "conference", "2412", "法說會")])
    t.assertTrue({"2317", "9999", "2882"}.isdisjoint({e["code"] for e in o["events"] if e["type"] in ("earnings", "conference")}))
    t.assertEqual((o["counts"]["earnings"], o["counts"]["conference"]), (2, 2))


def check_mops_month_fail_news_fallback(t, c):
    o = c.out
    t.assertEqual(o["errors"], ["法說會來源（MOPS）失敗：" + HTTP500])
    t.assertEqual(o["counts"]["earnings"], 0)
    t.assertEqual(events_of(o, "earnings"), [])
    t.assertEqual(events_of(o, "conference"), [
        ("2026-10-12", "conference", "2408", "法說會（線上）"), ("2026-10-13", "conference", "3008", "法說會（線上）"),
        ("2026-10-14", "conference", "9958", "法說會"), ("2026-10-15", "conference", "2330", "法說會")])
    t.assertEqual(o["counts"]["conference"], 4)
    t.assertTrue({"1101", "1216", "2603"}.isdisjoint({e["code"] for e in o["events"] if e["type"] == "conference"}))


def check_top100_empty_conference_fallback(t, c):
    o = c.out
    t.assertEqual(o["errors"], ["市值前百大來源失敗：市值一家都算不出來（來源欄位可能改名）",
                                "法說會：無市值前百大名單可篩，本輪改用備援來源"])
    t.assertEqual((o["top100"], o["top100_date"]), ([], None))
    t.assertEqual(o["counts"]["earnings"], 0)
    t.assertEqual(events_of(o, "conference"), [("2026-10-12", "conference", "2408", "法說會（線上）"),
                                               ("2026-10-15", "conference", "2330", "法說會")])
    t.assertEqual(o["margin"]["date"], "2026-10-02")   # STOCK_DAY_ALL 沒被動到，券資比照算


def check_top100_same_day_cache(t, c):
    o = c.out
    t.assertEqual(o["errors"], [])
    t.assertEqual((o["top100"], o["top100_date"]), (["2330", "2317", "2454"], "2026-10-05"))
    t.assertEqual(events_of(o, "earnings"), [("2026-10-15", "earnings", "2330", "Q3 財報（線上）")])
    t.assertEqual((o["counts"]["earnings"], o["counts"]["conference"]), (1, 0))


def check_top100_cache_hit_not_fresh(t, c):
    o, old = c.out, c.old
    t.assertEqual((o["fetched"], o["updated"]), ("2026-10-04 06:00", "2026-10-05 01:12"))
    t.assertEqual(prefixes(o["errors"]), ["總經來源失敗（本週檔）", "除權息來源失敗", "股東會來源失敗", "法說會來源（MOPS）失敗",
                                          "法說會備援來源（重大訊息）失敗", "行情來源失敗（SOFR 3M／NY Fed）", "休市日曆來源失敗"])
    t.assertEqual((o["top100"], o["top100_date"]), (["2330", "2317", "2454"], "2026-10-05"))
    t.assertEqual(o["macro"], old["macro"])
    t.assertEqual(o["holidays"], old["holidays"])
    t.assertEqual(o["events"], old["events"])
    t.assertEqual(quote_of(o, "SOFR 3M"), quote_of(old, "SOFR 3M"))


def check_top100_fail_keeps_old_date(t, c):
    o = c.out
    t.assertEqual(o["errors"], ["市值前百大來源失敗：" + HTTP500])
    t.assertEqual((o["top100"], o["top100_date"]), (["2330", "2308", "2412", "2317"], "2026-10-04"))
    t.assertEqual(events_of(o, "earnings"), [("2026-10-05", "earnings", "2308", "Q2 財報"),
                                             ("2026-10-15", "earnings", "2330", "Q3 財報（線上）")])
    t.assertEqual(o["fetched"], "2026-10-05 01:12")   # 其他來源照常抓到新資料


def check_macro_this_week_fail_keeps_old(t, c):
    o = c.out
    t.assertEqual(o["errors"], ["總經來源失敗（本週檔）：" + HTTP500])
    t.assertEqual(o["macro"], c.old["macro"])
    t.assertEqual(len(o["macro"]), 6)
    t.assertNotIn("Core CPI m/m", [m["title_en"] for m in o["macro"]])
    t.assertEqual(o["macro"][0]["title"], "舊檔沿用的總經事件")


def check_dividend_rwd_fail_openapi(t, c):
    o = c.out
    t.assertEqual(o["errors"], [])
    t.assertEqual(events_of(o, "dividend"), [
        ("2026-10-08", "dividend", "2330", "除息 現金6元"), ("2026-10-09", "dividend", "2317", "除權息 現金0.5元"),
        ("2026-10-13", "dividend", "1101", "除權"), ("2026-10-14", "dividend", "2412", "除權息 現金1.23457元"),
        ("2026-10-15", "dividend", "2881", "除 息 現金100元"), ("2026-10-16", "dividend", "2882", "除息"),
        ("2026-10-17", "dividend", "2603", "除息 現金1e-05元")])
    t.assertEqual(o["counts"]["dividend"], 7)   # 錄製 RWD 的 51 筆都不在


def check_quote_single_failure_keeps_old(t, c):
    o, old = c.out, c.old
    t.assertEqual(o["errors"], ["行情來源失敗（日經225／^N225）：" + HTTP500, "行情來源失敗（KOSPI／^KS11）：" + HTTP500])
    names = [q["name"] for q in o["quotes"]]
    t.assertEqual(names, ["USD/TWD", "US 10Y", "WTI 原油", "加權指數", "台積電", "日經225", "歐股50", "道瓊", "S&P 500",
                          "NASDAQ", "費半", "SOFR 3M"])
    t.assertEqual(quote_of(o, "日經225"), quote_of(old, "日經225"))
    t.assertEqual(quote_of(o, "日經225")["price"], 1.0)
    t.assertEqual(names.index("日經225"), 5)
    t.assertEqual(o["counts"]["quotes"], 12)


def check_quote_prev_close_intraday_extra_point(t, c):
    o = c.out
    t.assertEqual(o["errors"], [])
    usd, wti, nik = quote_of(o, "USD/TWD"), quote_of(o, "WTI 原油"), quote_of(o, "日經225")
    t.assertEqual((usd["price"], usd["prev"]), (31.5, 31.4))   # 不是 chartPreviousClose 的 99.0
    t.assertEqual((wti["price"], wti["prev"]), (70.5, 70.4))   # 不是當日 K 的 70.45，也不是 12.34
    # 正時差（gmtoffset 32400）、當日 K 已有收盤值 100.5＋即時報價 101.0：以 UTC 歸日會得 100.5，依當地日期歸戶才得 100.0
    t.assertEqual((nik["price"], nik["prev"]), (101.0, 100.0))
    for q in (usd, wti, nik):
        t.assertEqual(q["chg_pct"], (q["price"] / q["prev"] - 1) * 100)
        t.assertEqual(q["chg_abs"], q["price"] - q["prev"])
    t.assertEqual(quote_of(o, "KOSPI"), quote_of(c.rec, "KOSPI"))   # 其餘不受影響


def check_quote_prev_close_fallbacks(t, c):
    o = c.out
    t.assertEqual(o["errors"], ["行情來源失敗（NASDAQ／^IXIC）：前一交易日收盤缺值或為 0",
                                "行情來源失敗（費半／^SOX）：regularMarketPrice 缺值"])
    t.assertEqual(quote_of(o, "US 10Y")["prev"], 5.0)
    t.assertEqual(quote_of(o, "S&P 500")["prev"], 101.0)
    t.assertEqual(quote_of(o, "道瓊")["prev"], 40.0)
    names = [q["name"] for q in o["quotes"]]
    t.assertNotIn("NASDAQ", names)
    t.assertNotIn("費半", names)
    t.assertEqual(o["counts"]["quotes"], 11)


def check_sofr_last5_fallback(t, c):
    o = c.out
    t.assertEqual(o["errors"], [])
    sofr = o["quotes"][-1]
    t.assertEqual(sofr["name"], "SOFR 3M")
    t.assertEqual((sofr["price"], sofr["prev"]), (3.7, 3.6))
    t.assertEqual(sofr["t"], 1790899200)   # 2026-10-02 UTC 00:00


def check_sofr_main_500_no_fallback(t, c):
    o = c.out
    t.assertEqual(o["errors"], ["行情來源失敗（SOFR 3M／NY Fed）：" + HTTP500])
    t.assertEqual(o["quotes"][-1], c.old["quotes"][-1])
    t.assertEqual(o["quotes"][-1]["price"], 1.11)   # 打了 last/5 備援才會是 3.7
    t.assertEqual(o["counts"]["quotes"], 13)


def check_holiday_notice_rows(t, c):
    o = c.out
    t.assertEqual(o["errors"], [])
    t.assertEqual(o["holidays"], ["2026-02-12", "2026-05-01", "2026-06-19", "2026-09-25"])


def hhmm(ts):
    return datetime.fromtimestamp(ts, harness.TPE).strftime("%H:%M")


def check_twii_intraday_1200(t, c):
    o = c.out
    t.assertEqual((o["wallpaper_errors"], o["updated"]), ([], "2026-10-02 12:00"))
    ti = o["twii_intraday"]
    t.assertEqual(ti["date"], "2026-10-01")
    t.assertEqual((hhmm(ti["points"][0][0]), hhmm(ti["points"][-1][0]), len(ti["points"])), ("09:00", "13:25", 54))
    t.assertEqual(o["twii_daily"][-1]["date"], "2026-10-01")


def check_twii_intraday_1500(t, c):
    o = c.out
    t.assertEqual((o["wallpaper_errors"], o["updated"]), ([], "2026-10-02 15:00"))
    ti = o["twii_intraday"]
    t.assertEqual(ti["date"], "2026-10-02")
    t.assertEqual((hhmm(ti["points"][0][0]), hhmm(ti["points"][-1][0]), len(ti["points"])), ("09:00", "13:30", 55))
    t.assertEqual((o["twii_daily"][-1]["date"], o["twii_daily"][-1]["close"]), ("2026-10-02", 48475.74))


def check_twii_intraday_incomplete_keeps_old(t, c):
    o = c.out
    t.assertEqual(o["errors"], [])
    t.assertEqual(len(o["wallpaper_errors"]), 1)
    msg = o["wallpaper_errors"][0]
    t.assertTrue(msg.startswith("加權盤中走勢來源失敗：2026-10-02 序列終點"), msg)
    t.assertIn("太早（來源尚未追上收盤）", msg)
    t.assertEqual(o["twii_intraday"], c.old["twii_intraday"])
    t.assertEqual(o["twii_intraday"]["date"], "2026-10-01")


def check_twii_date_regression(t, c):
    o, old = c.out, c.old
    t.assertEqual(o["errors"], [])
    t.assertEqual(o["twii_daily"], old["twii_daily"])
    t.assertEqual(o["twii_daily"][-1]["date"], "2026-10-06")
    t.assertEqual(o["wallpaper_errors"],
                  ["加權日K來源回傳較舊的資料（最後一根 2026-10-02 早於上次的 2026-10-06），沿用上次資料"])
    t.assertEqual(o["twii_intraday"], old["twii_intraday"])   # 盤中走勢靜默沿用，沒有對應的錯誤


def check_margin_date_mismatch(t, c):
    o = c.out
    t.assertEqual(o["errors"], [])
    t.assertEqual(o["wallpaper_errors"], ["券資比／融資維持率來源失敗：彙總日期 2026-10-02 與個股明細日期 2026-10-01 不一致"])
    t.assertEqual(o["margin"], c.old["margin"])
    t.assertEqual(o["margin"]["date"], "2026-09-30")


def check_margin_price_date_mismatch(t, c):
    o = c.out
    t.assertEqual((o["errors"], o["wallpaper_errors"]), ([], []))   # 不記錯誤
    t.assertEqual(o["margin"], c.old["margin"])
    t.assertEqual(o["top100"], ["2330", "2317", "2454"])           # 前百大走快取，沒被極小價格表影響


def check_deliberate_old_wrong_types(t, c):
    o = c.out
    t.assertEqual((o["events"], o["punish"], o["top100"]), ([], [], []))
    t.assertIsNone(o["top100_date"])
    names = [q["name"] for q in o["quotes"]]
    t.assertNotIn("台積電", names)
    t.assertEqual(len(names), 12)
    t.assertIn("法說會：無市值前百大名單可篩，本輪改用備援來源", o["errors"])
    t.assertEqual(o["counts"]["punish"] + o["counts"]["dividend"] + o["counts"]["meeting"], 0)


def check_deliberate_old_wrong_types_passthrough(t, c):
    o = c.out
    t.assertEqual(o["macro"], [])
    t.assertEqual(o["counts"]["macro"], 0)
    t.assertNotIn("holidays", o)
    t.assertEqual(prefixes(o["errors"]), ["總經來源失敗（本週檔）", "休市日曆來源失敗"])


def check_deliberate_macro_nonstring_forecast(t, c):
    o = c.out
    t.assertEqual(o["errors"], [])
    t.assertEqual([(m["title_en"], m["forecast"], m["previous"]) for m in o["macro"]], [
        ("ISM Services PMI", "55.1", "55.4"), ("Final Manufacturing PMI", "", "49.8"), ("FOMC Member Speaks", "", ""),
        ("Retail Sales m/m", "", "0.1%")])   # null 與數字 0 照 Python 視為空字串、保留；只略過非空非字串的列
    t.assertEqual(o["counts"]["macro"], 4)


CHECKS = {
    "all-sources-fail-with-old": check_all_sources_fail_with_old,
    "all-sources-fail-no-old": check_all_sources_fail_no_old,
    "wallpaper-only-success": check_wallpaper_only_success,
    "punish-tpex-fail": check_punish_tpex_fail,
    "punish-tpex-blank-template": check_punish_tpex_blank_template,
    "punish-second-disposition": check_punish_second_disposition,
    "punish-period-formats": check_punish_period_formats,
    "conference-range-date": check_conference_range_date,
    "mops-month-fail-news-fallback": check_mops_month_fail_news_fallback,
    "top100-empty-conference-fallback": check_top100_empty_conference_fallback,
    "top100-same-day-cache": check_top100_same_day_cache,
    "top100-cache-hit-not-fresh": check_top100_cache_hit_not_fresh,
    "top100-fail-keeps-old-date": check_top100_fail_keeps_old_date,
    "macro-this-week-fail-keeps-old": check_macro_this_week_fail_keeps_old,
    "dividend-rwd-fail-openapi": check_dividend_rwd_fail_openapi,
    "quote-single-failure-keeps-old": check_quote_single_failure_keeps_old,
    "quote-prev-close-intraday-extra-point": check_quote_prev_close_intraday_extra_point,
    "quote-prev-close-fallbacks": check_quote_prev_close_fallbacks,
    "sofr-last5-fallback": check_sofr_last5_fallback,
    "sofr-main-500-no-fallback": check_sofr_main_500_no_fallback,
    "holiday-notice-rows": check_holiday_notice_rows,
    "twii-intraday-1200": check_twii_intraday_1200,
    "twii-intraday-1500": check_twii_intraday_1500,
    "twii-intraday-incomplete-keeps-old": check_twii_intraday_incomplete_keeps_old,
    "twii-date-regression": check_twii_date_regression,
    "margin-date-mismatch": check_margin_date_mismatch,
    "margin-price-date-mismatch": check_margin_price_date_mismatch,
    "deliberate-old-wrong-types": check_deliberate_old_wrong_types,
    "deliberate-old-wrong-types-passthrough": check_deliberate_old_wrong_types_passthrough,
    "deliberate-macro-nonstring-forecast": check_deliberate_macro_nonstring_forecast,
}


class BoundaryScenarioTest(unittest.TestCase):
    """host/tests/fixtures/fetch/<情境>/：expected.json 要與重跑 Python 相同，且要真的出現該分支的證據"""

    def test_every_scenario_directory_has_exactly_one_check(self):
        self.assertEqual(set(SCENARIOS), set(CHECKS), "情境目錄與 CHECKS 表不一致")
        self.assertGreaterEqual(len(SCENARIOS), 29)

    def test_scenario_files(self):
        for name in SCENARIOS:
            with self.subTest(scenario=name):
                d = FETCH_FIXTURES / name
                sc = json.loads((d / "scenario.json").read_text(encoding="utf-8"))
                self.assertEqual(sc["base"], "../recorded-20261005")        # 相對路徑，位置無關
                self.assertEqual("expected_from" in sc, name.startswith("deliberate-"))
                readme = (d / "README.md").read_text(encoding="utf-8")
                self.assertTrue(readme.startswith(f"# {name}\n"))
                self.assertIn("- 對應條目：", readme)
                self.assertIn("預期在 `expected.json` 看到的證據", readme)
                if name.startswith("deliberate-"):
                    self.assertIn("## 刻意改良情境", readme)
                    self.assertIn("Rust 版必須在本目錄原始輸入", readme)
                raw = (d / "expected.json").read_bytes()
                self.assertNotIn(b"\r", raw)
                self.assertTrue(raw.endswith(b"}\n"))
                for f in d.iterdir():   # 覆寫用的回應檔只放被修改的那幾個，不得整份複製大檔
                    limit = 120_000 if f.name in ("expected.json", "old.json") else 40_000
                    self.assertLess(f.stat().st_size, limit, f.name)

    def test_expected_json_equals_a_fresh_run_of_the_oracle(self):
        for name in SCENARIOS:
            with self.subTest(scenario=name):
                d = FETCH_FIXTURES / name
                fresh = harness.render(run_offline(d, for_expected=True))
                self.assertEqual((d / "expected.json").read_bytes().decode("utf-8"), fresh)

    def test_scenario_evidence(self):
        rec = json.loads((RECORDED[-1] / "expected.json").read_text(encoding="utf-8"))
        for name in SCENARIOS:
            with self.subTest(scenario=name):
                d = FETCH_FIXTURES / name
                out = json.loads((d / "expected.json").read_text(encoding="utf-8"))
                old = json.loads((d / "old.json").read_text(encoding="utf-8")) if (d / "old.json").exists() else None
                # 每個情境用到的請求都有對應回應（沒有「查無對應」＝沒有漏覆寫備援／絆線）
                self.assertFalse([e for e in out["errors"] + out["wallpaper_errors"] if "查無對應" in e])
                CHECKS[name](self, SimpleNamespace(out=out, old=old, rec=rec, d=d))

    def test_deliberate_original_input_does_not_produce_expected_json(self):
        """刻意改良情境：Python 在原始輸入下崩潰或輸出不同，所以期望檔來自等效乾淨輸入"""
        for name in (n for n in SCENARIOS if n.startswith("deliberate-")):
            with self.subTest(scenario=name):
                d = FETCH_FIXTURES / name
                expected = json.loads((d / "expected.json").read_text(encoding="utf-8"))
                try:
                    raw = run_offline(d)
                except (AttributeError, TypeError, KeyError) as e:
                    self.assertIsNotNone(e)   # 崩潰：K-14／D8-3，沒有原始輸出可當期望
                else:
                    self.assertNotEqual(raw, expected)
        # 崩潰的情境確實是崩潰、不是悄悄輸出別的東西
        for name in ("deliberate-macro-nonstring-forecast", "deliberate-old-wrong-types"):
            with self.assertRaises((AttributeError, TypeError)):
                run_offline(FETCH_FIXTURES / name)
        raw = run_offline(FETCH_FIXTURES / "deliberate-old-wrong-types-passthrough")
        self.assertEqual((raw["macro"], raw["holidays"], raw["counts"]["macro"]), ("oops", "oops", 4))

    def test_recorded_sample_covers_next_week_404_as_normal(self):
        """B-FF-2：平日下週總經檔 404 屬正常、不進 errors（由錄製樣本本身涵蓋；本週檔失敗見 macro-this-week-fail-keeps-old）"""
        manifest = json.loads((RECORDED[-1] / "manifest.json").read_text(encoding="utf-8"))
        nxt = next(r for r in manifest["requests"] if r["url"].endswith("ff_calendar_nextweek.json"))
        self.assertEqual(nxt["status"], 404)
        rec = json.loads((RECORDED[-1] / "expected.json").read_text(encoding="utf-8"))
        self.assertEqual(rec["errors"], [])
        self.assertTrue(rec["macro"])


if __name__ == "__main__":
    unittest.main()
