<#
.SYNOPSIS
  唯讀探針：列出宿主「顯示器穩定識別」對應邏輯用到的兩份 Win32 資料，並模擬宿主的對應結果。

.DESCRIPTION
  只呼叫查詢型 API，不改任何顯示設定、不啟動也不碰 fc-host、不注入輸入：
  1. QueryDisplayConfig（預設 QDC_ONLY_ACTIVE_PATHS，與 host/src/desktop.rs 的
     source_name_to_device_path 同一個旗標）的每條路徑：source／target 的 adapter LUID 與 id、
     DisplayConfigGetDeviceInfo 查得的 source GDI 名稱（viewGdiDeviceName）與 target
     monitorDevicePath（含兩次查詢各自的回傳碼）。
  2. EnumDisplayMonitors＋GetMonitorInfoW 的 szDevice、工作區、是否主螢幕（tao／Tauri 的
     Monitor.name 就是這個 szDevice）。
  3. 模擬 desktop.rs monitors_from_tauri：以 szDevice 精確比對（區分大小寫）第 1 步的
     「GDI 名稱 → device path」表，印出每台顯示器會得到的 id（查不到＝None＝宿主視為無穩定識別）。

  -AllPaths 改用 QDC_ALL_PATHS（含非作用中路徑，僅供對照；宿主不用這個旗標）。

.EXAMPLE
  pwsh -NoProfile -File host/tools/probe-monitor-ids.ps1
#>
[CmdletBinding()]
param(
    [switch]$AllPaths
)

$ErrorActionPreference = 'Stop'

# 型別定義已抽到 lib/MonitorIdsType.ps1（task 1.5 與 probe-dw-1.5.ps1 共用）。
. (Join-Path $PSScriptRoot 'lib\MonitorIdsType.ps1')

$flagName = if ($AllPaths) { 'QDC_ALL_PATHS' } else { 'QDC_ONLY_ACTIVE_PATHS' }
$paths = [FcProbe.MonitorIds]::Paths([bool]$AllPaths)
"== QueryDisplayConfig($flagName) status=$([FcProbe.MonitorIds]::LastQueryStatus) paths=$($paths.Count)"
foreach ($p in $paths) {
    "[path $($p.Index)] source adapter=$($p.SourceAdapter) id=$($p.SourceId) -> GDI='$($p.SourceGdiName)' (status=$($p.SourceStatus))"
    "          target adapter=$($p.TargetAdapter) id=$($p.TargetId) available=$($p.TargetAvailable) pathFlags=0x$('{0:X}' -f $p.PathFlags)"
    "          monitorDevicePath='$($p.MonitorDevicePath)' friendly='$($p.FriendlyName)' (status=$($p.TargetStatus))"
}

# 宿主的對照表：同一 GDI 名稱出現多次時，HashMap::insert 後者覆蓋前者——照樣模擬。
$map = @{}
$dupes = @()
foreach ($p in $paths) {
    if ($null -ne $p.SourceGdiName -and $null -ne $p.MonitorDevicePath) {
        if ($map.ContainsKey($p.SourceGdiName)) { $dupes += $p.SourceGdiName }
        $map[$p.SourceGdiName] = $p.MonitorDevicePath
    }
}
if ($dupes.Count -gt 0) { "!! 同一 GDI 名稱對到多條路徑（後者覆蓋前者）：$($dupes -join ', ')" }

$monitors = [FcProbe.MonitorIds]::Monitors()
""
"== EnumDisplayMonitors／GetMonitorInfoW: $($monitors.Count) 台"
foreach ($m in $monitors) {
    # PowerShell 的 hashtable 鍵不分大小寫；宿主的 HashMap 區分，故這裡用 -ceq 逐一精確比對。
    $hit = $map.GetEnumerator() | Where-Object { $_.Key -ceq $m.SzDevice } | Select-Object -First 1
    $id = if ($hit) { "Device('$($hit.Value)')" } else { 'None（宿主視為無穩定識別）' }
    "szDevice='$($m.SzDevice)' primary=$($m.Primary) work=$($m.Work) bounds=$($m.Bounds)"
    "          -> 宿主對應 id = $id"
}
