"""以樣本目錄（manifest）回放，輸出 expected.json（design.md D4）。

用法：uv run --no-project python tests/fetch_oracle/expect.py <情境目錄>

情境目錄可以是：
- 錄製樣本（manifest.json＋responses/）；
- 或 scenario.json：{"base": "<另一個情境目錄，相對於本目錄>", "overrides": [{method, url, form,
  status, reason, file 或 error}], "now_tpe": ISO, "now_local": ISO}。base 套用後再蓋上 overrides
  （鍵相同＝取代，否則新增）。回應檔路徑相對於「宣告它的那個目錄」。
  只給 now_tpe 時，本機時間取同一個台北牆上時間。
- 另可放 old.json，當作舊的 tw_events.json。
- scenario.json 可再加 "expected_from"（刻意改良情境用，design.md D8）：Python 在原始輸入下會崩潰或產生與 Rust 不同
  的結果，故 expected.json 改由「等效的乾淨輸入」產生。格式 {"overrides": [...], "old": "<檔名>" | null}：
  overrides 疊在本情境已解析好的請求表上（同鍵取代，檔案路徑相對於本情境目錄）；old 給檔名＝改用該檔當舊檔，
  給 null＝不放舊檔，省略＝沿用本情境的 old.json。只有 CLI／run_scenario(for_expected=True) 採用它；
  原始輸入仍可用 run_scenario(d) 回放（測試靠它確認原始輸入確實不會產生 expected.json）。

產生 expected.json 時（CLI／run_scenario(for_expected=True)）另外套用 Rust 版的刻意差異 first_presentation_only：
沿用更早簡報的法說會場次不算財報。run_scenario(d) 仍是 Python 版原樣的輸出。

回放規則：命中且 status 200 → 回傳位元組；命中但非 200 → HTTPError；命中 error 紀錄或查無 → URLError。
"""

from __future__ import annotations

import io
import json
import re
import sys
import urllib.error
from dataclasses import dataclass
from datetime import date
from email.message import Message
from http import client as http_client
from pathlib import Path

import harness


class RecordedURLError(urllib.error.URLError):
    """重現錄製時的連線層錯誤：str() 與當時的例外說明一致"""

    def __init__(self, message: str):
        super().__init__(message)
        self.message = message

    def __str__(self):
        return self.message


@dataclass
class Scenario:
    entries: dict      # 請求鍵 → 紀錄（含 "_dir"：回應檔相對的目錄）
    frozen: harness.FrozenTime
    old_path: Path | None


def _load_json(p: Path):
    return json.loads(p.read_text(encoding="utf-8"))


def _normalize(raw: dict, base_dir: Path) -> tuple[tuple, dict]:
    entry = dict(raw)
    entry.setdefault("form", None)
    entry["method"] = "GET" if entry["form"] is None else "POST"
    entry["_dir"] = base_dir
    return harness.request_key(entry["url"], entry["form"]), entry


def _resolve(d: Path) -> tuple[dict, str, str | None]:
    """情境目錄 → (請求表, now_tpe 字串, now_local 字串或 None)"""
    d = Path(d)
    sc_path = d / "scenario.json"
    scenario = _load_json(sc_path) if sc_path.exists() else {}
    if "base" in scenario:
        entries, now_tpe, now_local = _resolve(d / scenario["base"])
    elif (d / "manifest.json").exists():
        manifest = _load_json(d / "manifest.json")
        entries = dict(_normalize(r, d) for r in manifest["requests"])
        now_tpe, now_local = manifest["recorded_at_tpe"], manifest.get("recorded_at_local")
    else:
        raise FileNotFoundError(f"{d} 沒有 manifest.json，也沒有帶 base 的 scenario.json")
    for raw in scenario.get("overrides", []):
        key, entry = _normalize(raw, d)
        entries[key] = entry
    if "now_tpe" in scenario:
        now_tpe, now_local = scenario["now_tpe"], scenario.get("now_local")
    elif "now_local" in scenario:
        now_local = scenario["now_local"]
    return entries, now_tpe, now_local


EXPECTED_FROM_KEYS = {"overrides", "old"}


def load_scenario(d: Path, *, for_expected: bool = False) -> Scenario:
    """for_expected=True：套用 scenario.json 的 expected_from（等效乾淨輸入）；預設回放原始輸入"""
    d = Path(d)
    entries, now_tpe, now_local = _resolve(d)
    old_file = d / "old.json"
    old_path = old_file if old_file.exists() else None
    sc_path = d / "scenario.json"
    spec = _load_json(sc_path).get("expected_from") if sc_path.exists() else None
    if for_expected and spec is not None:
        unknown = set(spec) - EXPECTED_FROM_KEYS
        if unknown:
            raise ValueError(f"{sc_path} 的 expected_from 有不認得的鍵：{sorted(unknown)}")
        for raw in spec.get("overrides", []):
            key, entry = _normalize(raw, d)
            entries[key] = entry
        if "old" in spec:
            old_path = d / spec["old"] if spec["old"] else None
    return Scenario(entries, harness.parse_frozen(now_tpe, now_local), old_path)


def replay_factory(entries: dict):
    def factory(_orig):
        def replay(url, form=None):
            key = harness.request_key(url, form)
            entry = entries.get(key)
            if entry is None:
                raise urllib.error.URLError(f"查無對應的錄製回應：{key[0]} {url}")
            if "error" in entry:
                raise RecordedURLError(entry["error"].partition(": ")[2] or entry["error"])
            body = b""
            if entry.get("file"):
                body = (Path(entry["_dir"]) / entry["file"]).read_bytes()
            status = entry.get("status", 200)
            if status == 200:
                return body
            reason = entry.get("reason") or http_client.responses.get(status, "")
            raise urllib.error.HTTPError(url, status, reason, Message(), io.BytesIO(body))

        return replay

    return factory


# MOPS 簡報檔名：代號 4 碼＋西元年月日＋語言代碼（M 中文、E 英文），例 301720261110M002.pdf
RE_DECK = re.compile(r"^[0-9]{4}([0-9]{4})([0-9]{2})([0-9]{2})[ME]")


def deck_date(file_name: str) -> date | None:
    m = RE_DECK.match(file_name)
    if not m:
        return None
    try:
        return date(*(int(g) for g in m.groups()))
    except ValueError:
        return None


def first_presentation_only(mod) -> None:
    """Rust 版的刻意差異（host/src/fetch/conference.rs 的 reuses_earlier_deck）：簡報欄（第 7、8 欄）第一個認得出
    日期的檔名早於召開起日＝財報公布後沿用舊簡報的後續場次，不算財報。

    Python 版只看擇要訊息有沒有季別，所以這裡把這類列的擇要訊息換成不含季別的字（保留「線上」供備註判斷），
    其餘照 Python 版原樣執行。認不出檔名的列（未來場次的「會後公告」、空白）不動。"""
    mops_rows = mod.mops_rows

    def patched(text: str) -> list[list[str]]:
        rows = mops_rows(text)
        for r in rows:
            start = mod.roc_to_date(r[2].split("至")[0].strip()) if len(r) >= 7 else None
            deck = next((d for d in map(deck_date, r[6:8]) if d), None)
            if start and deck and deck < start:
                r[5] = "線上" if "線上" in r[5] else ""
        return rows

    mod.mops_rows = patched


def run_scenario(d: Path, *, for_expected: bool = False) -> harness.RunResult:
    sc = load_scenario(d, for_expected=for_expected)
    patch = first_presentation_only if for_expected else None
    return harness.run_isolated(sc.frozen, replay_factory(sc.entries), sc.old_path, patch)


def main(argv=None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    if len(argv) != 1:
        print("用法：expect.py <情境目錄>", file=sys.stderr)
        return 2
    d = Path(argv[0])
    result = run_scenario(d, for_expected=True)
    harness.write_lf(d / "expected.json", harness.render(result.output))
    print(f"已輸出 → {d / 'expected.json'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
