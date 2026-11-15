# fetch-progress-and-manual-refresh task 5.4：以 UI Automation 找出通知區域裡提示文字以「財經日曆」開頭的圖示，
# 記下它的名稱（＝提示文字）並截下該圖示（原尺寸與放大 8 倍各一張）。不注入滑鼠鍵盤：圖示收在溢位區時以 UIA 的
# Invoke／ExpandCollapse 展開。
param([Parameter(Mandatory)][string]$Label, [string]$OutDir = $PSScriptRoot, [int]$Margin = 36)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Drawing
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class TbDpi {
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
}
'@
[TbDpi]::SetProcessDpiAwarenessContext([IntPtr](-4)) | Out-Null
if (Get-Process LogonUI, LockApp -ErrorAction SilentlyContinue) { Write-Output 'BLOCKED: 工作階段鎖定中'; exit 2 }

$auto = [System.Windows.Automation.AutomationElement]
$tree = [System.Windows.Automation.TreeScope]
function Find-Icon {
    $roots = $auto::RootElement.FindAll($tree::Children, [System.Windows.Automation.Condition]::TrueCondition) |
        Where-Object { $_.Current.ClassName -in 'Shell_TrayWnd', 'TopLevelWindowForOverflowXamlIsland', 'NotifyIconOverflowWindow' }
    foreach ($r in $roots) {
        foreach ($e in $r.FindAll($tree::Descendants, [System.Windows.Automation.Condition]::TrueCondition)) {
            if ($e.Current.Name -like '財經日曆*') { return $e }
        }
    }
    return $null
}
$icon = Find-Icon
$where = '工作列'
if (-not $icon -or $icon.Current.IsOffscreen) {
    $tray = $auto::RootElement.FindFirst($tree::Children, (New-Object System.Windows.Automation.PropertyCondition($auto::ClassNameProperty, 'Shell_TrayWnd')))
    $chev = $tray.FindAll($tree::Descendants, [System.Windows.Automation.Condition]::TrueCondition) |
        Where-Object { $_.Current.Name -match '隱藏的圖示|hidden icons' } | Select-Object -First 1
    if (-not $chev) { Write-Output 'FAIL: 找不到圖示，也找不到溢位區按鈕'; exit 1 }
    # 按鈕是切換式：上一次執行留著展開的溢位區時，第一次按會收起來，所以最多按兩次。
    foreach ($try in 1, 2) {
        $p = $null
        if ($chev.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$p)) { $p.Invoke() }
        elseif ($chev.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern, [ref]$p)) { $p.Toggle() }
        Start-Sleep -Milliseconds 1500
        $icon = Find-Icon
        if ($icon -and -not $icon.Current.IsOffscreen) { break }
    }
    $where = '溢位區'
}
if (-not $icon) { Write-Output 'FAIL: 展開溢位區後仍找不到圖示'; exit 1 }
$r = $icon.Current.BoundingRectangle
$dpi = [TbDpi]::GetDpiForWindow([IntPtr]$auto::RootElement.FindFirst($tree::Children, (New-Object System.Windows.Automation.PropertyCondition($auto::ClassNameProperty, 'Shell_TrayWnd'))).Current.NativeWindowHandle)
# UIA 回報的矩形與實際繪製位置在溢位區會差半格，四周各多截 $Margin 像素。
$x = [int]$r.X - $Margin; $y = [int]$r.Y - $Margin; $w = [int]$r.Width + 2 * $Margin; $h = [int]$r.Height + 2 * $Margin
$bmp = New-Object System.Drawing.Bitmap $w, $h
$g = [System.Drawing.Graphics]::FromImage($bmp); $g.CopyFromScreen($x, $y, 0, 0, $bmp.Size); $g.Dispose()
$raw = Join-Path $OutDir "fetch-progress-5.4-tray-$Label.png"; $bmp.Save($raw)
$big = New-Object System.Drawing.Bitmap ($w * 8), ($h * 8)
$g2 = [System.Drawing.Graphics]::FromImage($big); $g2.InterpolationMode = 'NearestNeighbor'; $g2.PixelOffsetMode = 'Half'
$g2.DrawImage($bmp, 0, 0, $w * 8, $h * 8); $g2.Dispose()
$big.Save((Join-Path $OutDir "fetch-progress-5.4-tray-$Label-x8.png")); $big.Dispose(); $bmp.Dispose()
Write-Output "OK label=$Label 位置=$where 名稱=「$($icon.Current.Name)」 矩形=${w}x${h} 工作列DPI=$dpi（$([int]($dpi / 96 * 100))%）"
