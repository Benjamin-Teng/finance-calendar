<#
.SYNOPSIS
    驗收腳本自己的「一般應用程式視窗」：另起一個 pwsh 行程跑 WinForms 表單（含多行 TextBox，可取得
    鍵盤焦點），收尾只關這扇表單與這個行程（fix F7）。

.DESCRIPTION
    不要借用記事本：Win11 記事本是單一行程多視窗，`Start-Process notepad` 拿到的 PID 只是啟動殼，
    新視窗落在使用者既有的 Notepad 行程裡；以 PID 收尾會連使用者原本開著的記事本視窗一起關掉
    （memory：win11-notepad-single-process-multi-window）。

    - `Get-ScratchFormCommand -Title <標題>`：純函式，回傳給 `pwsh -Command` 的指令字串。先
      `Show()`／`Hide()` 用掉 STARTUPINFO 的 SW_HIDE（`-WindowStyle Hidden` 只讓 pwsh 主控台隱藏），
      再 `Application.Run` 顯示表單。
    - `Start-ScratchForm [-Title <前綴>] [-X -Y -Width -Height]`：啟動表單、等視窗出現（以行程的
      MainWindowHandle＋標題比對），可選以 `SetWindowPos`（呼叫端執行緒的 DPI 座標；驗收腳本多為
      Per-Monitor-V2＝實體像素）擺位。回傳 `@{ Process; Hwnd; Title; ThreadId }`；逾時丟例外（行程已
      先結束）。
    - `Stop-ScratchForm <物件>`：對自己的 HWND `PostMessage(WM_CLOSE)`，等行程結束；逾時才結束
      **這個自己啟動的 pwsh 行程**（`$Form.Process`）。不以名稱、也不以視窗所屬 PID 結束任何行程。
#>
Set-StrictMode -Version Latest

if (-not ('FcScratch.Native' -as [type])) {
    Add-Type -Namespace FcScratch -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool PostMessageW(System.IntPtr h, uint msg, System.IntPtr w, System.IntPtr l);
[DllImport("user32.dll")] public static extern bool SetWindowPos(System.IntPtr h, System.IntPtr after, int x, int y, int cx, int cy, uint flags);
[DllImport("user32.dll")] public static extern bool IsWindow(System.IntPtr h);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr h, out uint pid);
'@
}

function Get-ScratchFormCommand {
    param([Parameter(Mandatory)][string]$Title)
    $quoted = "'" + ($Title -replace "'", "''") + "'"
    return "Add-Type -AssemblyName System.Windows.Forms; `$f = New-Object Windows.Forms.Form; `$f.Text = $quoted; " +
    "`$t = New-Object Windows.Forms.TextBox; `$t.Multiline = `$true; `$t.Dock = 'Fill'; `$f.Controls.Add(`$t); " +
    "`$f.Show(); `$f.Hide(); [Windows.Forms.Application]::Run(`$f)"
}

function Start-ScratchForm {
    param(
        [string]$Title = ('fc-host-scratch-' + [guid]::NewGuid().ToString('N').Substring(0, 8)),
        [int]$X = [int]::MinValue, [int]$Y = 0, [int]$Width = 500, [int]$Height = 400,
        [int]$TimeoutSec = 25
    )
    $proc = Start-Process pwsh -PassThru -WindowStyle Hidden -ArgumentList @('-NoProfile', '-Command', (Get-ScratchFormCommand -Title $Title))
    $hwnd = [IntPtr]::Zero
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $TimeoutSec) {
        Start-Sleep -Milliseconds 200
        $proc.Refresh()
        if ($proc.HasExited) { break }
        if ($proc.MainWindowHandle -ne [IntPtr]::Zero -and $proc.MainWindowTitle -eq $Title) { $hwnd = $proc.MainWindowHandle; break }
    }
    if ($hwnd -eq [IntPtr]::Zero) {
        $form = [PSCustomObject]@{ Process = $proc; Hwnd = [IntPtr]::Zero; Title = $Title; ThreadId = 0 }
        Stop-ScratchForm $form
        throw "自己的表單視窗未出現（$Title）"
    }
    if ($X -ne [int]::MinValue) {
        [void][FcScratch.Native]::SetWindowPos($hwnd, [IntPtr]::Zero, $X, $Y, $Width, $Height, 0x0014)  # NOZORDER|NOACTIVATE
    }
    $p = [uint32]0
    $tid = [FcScratch.Native]::GetWindowThreadProcessId($hwnd, [ref]$p)
    return [PSCustomObject]@{ Process = $proc; Hwnd = $hwnd; Title = $Title; ThreadId = [uint32]$tid }
}

function Stop-ScratchForm {
    param($Form, [int]$TimeoutSec = 5)
    if (-not $Form) { return }
    if ($Form.Hwnd -ne [IntPtr]::Zero -and [FcScratch.Native]::IsWindow($Form.Hwnd)) {
        [void][FcScratch.Native]::PostMessageW($Form.Hwnd, 0x10, [IntPtr]::Zero, [IntPtr]::Zero)  # WM_CLOSE
    }
    if ($Form.Process -and -not $Form.Process.HasExited) {
        if (-not $Form.Process.WaitForExit($TimeoutSec * 1000)) {
            # 只結束自己啟動的那個 pwsh 行程（Start-Process -PassThru 取得），不碰其他行程。
            Stop-Process -Id $Form.Process.Id -Force -ErrorAction SilentlyContinue
        }
    }
}

Export-ModuleMember -Function Get-ScratchFormCommand, Start-ScratchForm, Stop-ScratchForm
