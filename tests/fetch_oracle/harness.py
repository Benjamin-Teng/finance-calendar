"""Python oracle 的共用「隔離執行」包裝（design.md D4）。

`run_isolated` 把 update_tw_events.py 複製到全新的暫存資料夾、從那份複本載入成獨立模組，
替換掉所有會碰到外界的東西（Lively 鏡像、等網路、取時間、網路請求），再呼叫 main()，
讀暫存資料夾裡的 tw_events.json 當輸出。repo 與使用者的 Lively 資料夾都不會被寫到；
執行前後會比對這些位置的 tw_events.* 來證明這一點。

不修改 update_tw_events.py，全部靠「複製＋替換模組屬性」。
"""

from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import shutil
import sys
import tempfile
import urllib.parse
import uuid
from collections.abc import Callable
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any
from unittest import mock

REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT = REPO_ROOT / "update_tw_events.py"
TPE = timezone(timedelta(hours=8))

# 一個「http_bytes 實作」的工廠：吃複本模組原本的 http_bytes，回傳要換上去的實作
HttpFactory = Callable[[Callable[..., bytes]], Callable[..., bytes]]


@dataclass(frozen=True)
class FrozenTime:
    now_tpe: datetime    # 帶 +08:00 的台北時間（B-DATE-2：窗口、前百大、MOPS 月份、盤中走勢）
    now_local: datetime  # naive 本機時間（B-DATE-3：updated／fetched）

    def __post_init__(self):
        if self.now_tpe.tzinfo is None:
            raise ValueError("now_tpe 必須帶時區")
        if self.now_local.tzinfo is not None:
            raise ValueError("now_local 必須是 naive（本機牆上時間）")


def parse_frozen(now_tpe: str, now_local: str | None = None) -> FrozenTime:
    """ISO 字串 → FrozenTime。now_local 省略時取「台北牆上時間」（本機視同台北），結果與機器時區無關。"""
    tpe = datetime.fromisoformat(now_tpe)
    if tpe.tzinfo is None:
        tpe = tpe.replace(tzinfo=TPE)
    tpe = tpe.astimezone(TPE)
    local = datetime.fromisoformat(now_local) if now_local else tpe.replace(tzinfo=None)
    return FrozenTime(tpe, local)


def request_key(url: str, form) -> tuple:
    """請求的比對鍵：(方法, 網址, 編碼後的表單)。form 可以是 None、dict 或 [[k, v], ...]。"""
    if form is None:
        return ("GET", url, None)
    return ("POST", url, urllib.parse.urlencode([tuple(p) for p in (form.items() if isinstance(form, dict) else form)]))


def _frozen_datetime_class(base: Any, frozen: FrozenTime) -> Any:
    class FrozenDatetime(base):
        @classmethod
        def now(cls, tz=None):
            src = frozen.now_local if tz is None else frozen.now_tpe.astimezone(tz)
            return cls(src.year, src.month, src.day, src.hour, src.minute, src.second,
                       src.microsecond, tzinfo=src.tzinfo)

    return FrozenDatetime


def snapshot_outputs(dirs) -> dict:
    """各資料夾內 tw_events.* 的 (修改時間, 大小)，用來證明沒有任何輸出落到暫存夾以外"""
    snap = {}
    for d in dirs:
        for p in sorted(Path(d).glob("tw_events.*")):
            st = p.stat()
            snap[str(p)] = (st.st_mtime_ns, st.st_size)
    return snap


class IsolationLeak(RuntimeError):
    """tw_events.* 出現在 repo 根目錄或 Lively 夾（oracle 的隔離失效）"""


@dataclass
class RunResult:
    output: dict
    log: str
    returncode: int


def run_isolated(frozen: FrozenTime, http_factory: HttpFactory, old_path: Path | None = None) -> RunResult:
    """在隔離環境跑一輪 main()，回傳輸出（tw_events.json 解析後）與 log。

    old_path：當作舊輸出（暫存夾裡的 tw_events.json）；None＝沒有舊檔。
    """
    tmp = Path(tempfile.mkdtemp(prefix="fc-oracle-"))
    try:
        script = tmp / "update_tw_events.py"
        shutil.copyfile(SCRIPT, script)
        if old_path is not None:
            shutil.copyfile(old_path, tmp / "tw_events.json")

        name = f"update_tw_events_oracle_{uuid.uuid4().hex[:8]}"
        spec = importlib.util.spec_from_file_location(name, script)
        assert spec is not None and spec.loader is not None
        mod: Any = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)

        # 隔離前先記下「原始」lively_wallpaper_dirs 會回傳的位置，連同 repo 根目錄一起監看
        watched = [REPO_ROOT, *mod.lively_wallpaper_dirs()]
        before = snapshot_outputs(watched)

        mod.lively_wallpaper_dirs = list
        mod.wait_for_network = lambda *a, **k: True
        mod.datetime = _frozen_datetime_class(mod.datetime, frozen)
        mod.http_bytes = http_factory(mod.http_bytes)   # http_json 在呼叫時才解析 http_bytes，一併生效

        buf = io.StringIO()
        with mock.patch.object(sys, "argv", [str(script)]), contextlib.redirect_stdout(buf):
            rc = mod.main()

        after = snapshot_outputs(watched)
        if after != before:
            raise IsolationLeak(f"隔離失效：tw_events.* 有變動 before={before} after={after}")

        out_path = tmp / "tw_events.json"
        if not out_path.exists():
            raise RuntimeError(f"main() 回傳 {rc}，暫存夾沒有 tw_events.json\n{buf.getvalue()}")
        output = json.loads(out_path.read_text(encoding="utf-8"))
        return RunResult(output, buf.getvalue(), rc)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def write_lf(path: Path, text: str) -> None:
    """寫 UTF-8、換行固定 LF（Windows 的 write_text 會轉成 CRLF，樣本檔要跨機器位元組一致）"""
    path.write_bytes(text.encode("utf-8"))


def render(output: dict) -> str:
    """expected.json／live_output.json 的統一寫法"""
    return json.dumps(output, ensure_ascii=False, indent=1) + "\n"
