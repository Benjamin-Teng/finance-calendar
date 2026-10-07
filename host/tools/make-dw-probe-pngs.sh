#!/usr/bin/env bash
# dynamic-wallpaper 探針（probe_wallpaper）的測試圖產生器：以星盤樣稿 bg-sessions.html 在兩個
# 不同時刻各畫一張 3840x2160 canvas，直接匯出 canvas 像素成 PNG（不截圖——headless Edge 的
# viewport 比視窗小約 30px，截圖邊緣會有接縫）。輸出到 %LOCALAPPDATA%\fc-probe-wallpaper\{a,b}.png，
# PNG 不進 git。用法：bash host/tools/make-dw-probe-pngs.sh [輸出目錄]
# 需要 assets/design-explore/05-astrolabe/bg-sessions.html（assets/ 不進 git，本機才有）。
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd)"
src="$repo/assets/design-explore/05-astrolabe/bg-sessions.html"
[ -f "$src" ] || { echo "找不到 $src"; exit 1; }
out="${1:-$(cygpath -u "$LOCALAPPDATA")/fc-probe-wallpaper}"
mkdir -p "$out"
edge="/c/Program Files (x86)/Microsoft/Edge/Application/msedge.exe"
tmp="$(mktemp -d)"
winsrc="$(cygpath -m "$src")"

render() { # $1=t（本機時間）$2=輸出檔
  cat > "$tmp/export.html" <<HTML
<!DOCTYPE html><html><head><meta charset="UTF-8"></head><body><pre id="out"></pre>
<script>
var f = document.createElement('iframe');
f.style.cssText = 'position:fixed;left:0;top:0;border:0;width:3840px;height:2160px';
f.src = 'file:///$winsrc?layout=full&w=3840&h=2160&t=$1';
f.onload = function () {
  var d = f.contentDocument;
  (d.fonts ? d.fonts.ready : Promise.resolve()).then(function () {
    setTimeout(function () {
      var c = d.querySelector('canvas');
      document.getElementById('out').textContent = 'W=' + c.width + ' H=' + c.height + '\n' + c.toDataURL('image/png');
    }, 4000);
  });
};
document.body.appendChild(f);
</script></body></html>
HTML
  "$edge" --headless=new --disable-gpu --allow-file-access-from-files --virtual-time-budget=30000 \
    --window-size=1200,800 --user-data-dir="$(cygpath -w "$tmp/ud")" --dump-dom \
    "file:///$(cygpath -m "$tmp/export.html")" > "$tmp/dom.html" 2>/dev/null
  grep -o 'W=[0-9]* H=[0-9]*' "$tmp/dom.html" || { echo "FAIL：canvas 沒有匯出（$1）"; exit 1; }
  grep -o 'data:image/png;base64,[A-Za-z0-9+/=]*' "$tmp/dom.html" | sed 's/^data:image\/png;base64,//' | base64 -d > "$2"
}

render "2026-10-02T09:00" "$out/a.png"
render "2026-10-02T21:00" "$out/b.png"
rm -rf "$tmp"
ls -l "$out/a.png" "$out/b.png"
