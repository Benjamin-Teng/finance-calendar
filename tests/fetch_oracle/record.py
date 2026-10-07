"""實跑一輪 update_tw_events.main()，把每個網路請求與回應錄成樣本目錄（design.md D4）。

用法：uv run --no-project python tests/fetch_oracle/record.py [--out-root <資料夾>]

會發真實網路請求（約 20 個）。樣本目錄 = <out-root>/recorded-<台北日期 YYYYMMDD>/：
manifest.json、responses/NNN.<副檔名>、live_output.json（錄製當下 Python 的輸出）、README.md。
錄製本身也用凍結時間（錄製開始那一刻）跑，所以同一份樣本回放可得到完全相同的輸出。
"""

from __future__ import annotations

import argparse
import json
import sys
import urllib.error
from datetime import datetime
from pathlib import Path

import harness

DEFAULT_OUT_ROOT = harness.REPO_ROOT / "host" / "tests" / "fixtures" / "fetch"


def guess_ext(body: bytes) -> str:
    head = body.lstrip()[:200].lower()
    if head.startswith((b"{", b"[")):
        try:
            json.loads(body.decode("utf-8"))
            return "json"
        except ValueError:
            pass
    if b"<html" in head or b"<!doctype" in head or b"<table" in body[:4000].lower():
        return "html"
    return "txt"


class Recorder:
    """包住 http_bytes：先呼叫原本實作並記錄，結果（含例外）原樣交還。"""

    def __init__(self):
        self.entries: dict[tuple, dict] = {}        # 請求鍵 → 第一次的紀錄（保持首見順序）
        self.bodies: dict[tuple, bytes] = {}
        self.mismatches: list[str] = []

    def factory(self, orig):
        def wrapped(url, form=None):
            key = harness.request_key(url, form)
            entry = {
                "method": "GET" if form is None else "POST",
                "url": url,
                "form": None if form is None else [[k, v] for k, v in form.items()],
            }
            body = b""
            try:
                body = orig(url, form)
            except urllib.error.HTTPError as e:
                try:
                    body = e.read()
                except Exception:  # noqa: BLE001  讀不到本體就當空，原例外照常往外丟
                    body = b""
                entry.update(status=e.code, reason=str(e.reason))
                self._store(key, entry, body)
                raise
            except Exception as e:
                entry["error"] = f"{type(e).__name__}: {e}"
                self._store(key, entry, None)
                raise
            entry["status"] = 200
            self._store(key, entry, body)
            return body

        return wrapped

    def _store(self, key, entry, body):
        first = self.entries.get(key)
        if first is None:
            entry["count"] = 1
            self.entries[key] = entry
            self.bodies[key] = body if body is not None else b""
            return
        first["count"] += 1
        same = (first.get("status") == entry.get("status") and first.get("error") == entry.get("error")
                and (body is None or self.bodies[key] == body))
        if not same:
            self.mismatches.append(f"{entry['method']} {entry['url']}：同一輪第 {first['count']} 次回應與第一次不同")


def write_sample(out: Path, rec: Recorder, frozen: harness.FrozenTime, live: harness.RunResult):
    (out / "responses").mkdir(parents=True)
    requests = []
    for i, (key, entry) in enumerate(rec.entries.items(), start=1):
        entry = dict(entry)
        if "error" not in entry:   # 連線層失敗沒有回應本體，其餘（含 HTTP 錯誤）都存檔
            rel = f"responses/{i:03d}.{guess_ext(rec.bodies[key])}"
            (out / rel).write_bytes(rec.bodies[key])
            entry["file"] = rel
        requests.append(entry)
    manifest = {
        "recorded_at_tpe": frozen.now_tpe.isoformat(),
        "recorded_at_local": frozen.now_local.isoformat(),
        "requests": requests,
    }
    if rec.mismatches:
        manifest["duplicate_mismatches"] = rec.mismatches
    harness.write_lf(out / "manifest.json", json.dumps(manifest, ensure_ascii=False, indent=1) + "\n")
    harness.write_lf(out / "live_output.json", harness.render(live.output))
    harness.write_lf(out / "README.md", readme(frozen, requests, rec))


def readme(frozen: harness.FrozenTime, requests: list, rec: Recorder) -> str:
    failed = [r for r in requests if r.get("status") != 200]
    lines = [
        f"# 錄製樣本 recorded-{frozen.now_tpe:%Y%m%d}",
        "",
        ("`tests/fetch_oracle/record.py` 實跑 `update_tw_events.py` 一輪錄下的真實來源回應（design.md D4）。"
         "自動產生，請勿手改；重錄請新增另一個日期的目錄。"),
        "",
        f"- 錄製時間（台北）：{frozen.now_tpe.isoformat()}",
        f"- 錄製時間（本機，naive）：{frozen.now_local.isoformat()}",
        (f"- 不同請求數：{len(requests)}（同一請求多次只留一筆，次數見 manifest 的 `count`）；"
         f"失敗 {len(failed)} 筆"),
        "- `live_output.json`：錄製當下 Python 版的輸出；回放（`expect.py`）必須與它完全相同。",
        "",
    ]
    if failed:
        lines += ["## 失敗的請求", ""]
        for r in failed:
            what = f"HTTP {r['status']}" if "status" in r else r["error"]
            lines.append(f"- `{r['method']} {r['url']}`：{what}")
        lines += ["",
                  ("平日 ForexFactory 下週檔回 404 屬正常（週末才發布）；其餘失敗代表當時來源不可用或被限流，"
                   "照實保留，回放時重現同樣的錯誤。"), ""]
    if rec.mismatches:
        lines += ["## 同一輪內回應不一致的重複請求", ""] + [f"- {m}" for m in rec.mismatches] + [""]
    return "\n".join(lines)


def main(argv=None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out-root", type=Path, default=DEFAULT_OUT_ROOT)
    args = ap.parse_args(argv)

    now_tpe = datetime.now(harness.TPE).replace(microsecond=0)
    frozen = harness.FrozenTime(now_tpe, now_tpe.astimezone().replace(tzinfo=None))
    out = args.out_root / f"recorded-{now_tpe:%Y%m%d}"
    if out.exists():
        print(f"目錄已存在，不覆蓋：{out}", file=sys.stderr)
        return 2

    rec = Recorder()
    live = harness.run_isolated(frozen, rec.factory)
    write_sample(out, rec, frozen, live)
    failed = [e for e in rec.entries.values() if e.get("status") != 200]
    print(f"已錄製 {len(rec.entries)} 個不同請求（失敗 {len(failed)}）→ {out}")
    for e in failed:
        print("  失敗：", e["method"], e["url"], e.get("status") or e.get("error"))
    for m in rec.mismatches:
        print("  不一致：", m)
    print(live.log)
    return 0


if __name__ == "__main__":
    sys.exit(main())
