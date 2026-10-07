"""產生 host/src/fetch/html_entities.rs：Python `html.entities.html5` 的靜態表（2231 筆）。

用法（在 repo 根目錄；Python 版本與 Unicode 版本會寫進檔頭）：

    uv run --no-project python tests/fetch_oracle/gen_html_entities.py

輸出固定寫到 host/src/fetch/html_entities.rs（LF 換行、依鍵排序以便 `binary_search`）。
鍵含結尾分號與舊式不帶分號的名稱（如 `amp;` 與 `amp`），值是 1～2 個碼位的字串。
"""

import html.entities
import pathlib
import sys
import unicodedata

BS = chr(92)
OUT = pathlib.Path(__file__).resolve().parents[2] / "host" / "src" / "fetch" / "html_entities.rs"


def rust_str(s: str) -> str:
    out = []
    for ch in s:
        o = ord(ch)
        if ch == '"':
            out.append(BS + '"')
        elif ch == BS:
            out.append(BS + BS)
        elif 0x20 <= o < 0x7F:
            out.append(ch)
        else:
            out.append(BS + f"u{{{o:x}}}")
    return '"' + "".join(out) + '"'


def main() -> None:
    table = html.entities.html5
    keys = sorted(table)
    py_ver = sys.version.split()[0]
    uni_ver = unicodedata.unidata_version
    lines = [
        "// 由腳本產生，勿手改。",
        "// 產生指令（repo 根目錄）：uv run --no-project python tests/fetch_oracle/gen_html_entities.py",
        f"// 來源：Python html.entities.html5（Python {py_ver}，Unicode {uni_ver}），共 {len(keys)} 筆。",
        "//",
        "// 依鍵的位元組序排序（供 binary_search）；鍵含結尾分號與舊式不帶分號的名稱。",
        "",
        f"pub static HTML5_ENTITIES: [(&str, &str); {len(keys)}] = [",
    ]
    for k in keys:
        lines.append(f"    ({rust_str(k)}, {rust_str(table[k])}),")
    lines.append("];")
    OUT.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    print("wrote", OUT, len(keys))


if __name__ == "__main__":
    main()
