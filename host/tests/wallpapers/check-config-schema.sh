#!/usr/bin/env bash
# host/tests/wallpapers/check-config-schema.sh
#
# 以 JSON schema 驗證主題設定檔預設值（task 3.2）：預設檔必須通過、config-invalid/ 下每份故意錯誤的樣本
# 必須被擋下。check-jsonschema 是一次性執行的工具（uvx），不裝進專案。
#   bash host/tests/wallpapers/check-config-schema.sh
set -u
export PYTHONUTF8=1
here="$(cd "$(dirname "$0")" && pwd)"
schema="$here/../../ui/wallpapers/config/wallpaper-config.schema.json"
default="$here/../../ui/wallpapers/config/wallpaper-config.default.json"
fail=0

echo "== schema 本身符合 draft 2020-12 =="
uvx check-jsonschema --check-metaschema "$schema" || fail=1

echo "== 預設檔必須通過 =="
uvx check-jsonschema --schemafile "$schema" "$default" || fail=1

echo "== 合法的使用者編輯（config-valid）必須通過 =="
for f in "$here"/config-valid/*.json; do
  if uvx check-jsonschema --schemafile "$schema" "$f" >/dev/null 2>&1; then
    echo "accepted: $(basename "$f")"
  else
    echo "WRONGLY REJECTED: $(basename "$f")"
    fail=1
  fi
done

echo "== 錯誤樣本必須被擋下 =="
for f in "$here"/config-invalid/*.json; do
  if uvx check-jsonschema --schemafile "$schema" "$f" >/dev/null 2>&1; then
    echo "NOT REJECTED: $(basename "$f")"
    fail=1
  else
    echo "rejected: $(basename "$f")"
  fi
done

if [ "$fail" -eq 0 ]; then echo "PASS"; else echo "FAIL"; fi
exit "$fail"
