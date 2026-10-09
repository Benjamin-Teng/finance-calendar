<#
.SYNOPSIS
    以 UTF-8 讀取 node 子行程的輸出，不依賴呼叫端主控台碼頁（widget-font-scale-per-widget task 5.2 實跑發現）。

.DESCRIPTION
    `& node ... 2>&1` 由 PowerShell 以 `[Console]::OutputEncoding` 解碼子行程輸出；從背景或非互動 shell 執行時
    那是系統碼頁（繁中 Windows＝950），node 寫出的 UTF-8 中文就被解成亂碼。CDP 回傳的 JSON 一含中文（例如宿主的
    錯誤訊息）就可能連引號一起被吃掉，ConvertFrom-Json 失敗或欄位讀不到——實跑 verify-adaptive-zoom.ps1 時
    `adjust_widget_font_scale` 的拒絕訊息因此讀不到 `ok`。

    `Invoke-NodeUtf8 -ArgumentList <參數...> [-TimeoutSec <秒>]`：以 System.Diagnostics.Process 啟動 node
    （ArgumentList 逐一傳參數，跳脫交給 .NET），stdout／stderr 以 UTF-8 非同步讀完（避免管線塞滿互等），回傳
    `{ ExitCode; StdOut; StdErr; Pid; TimedOut }`（字串已 Trim）。逾時就以 Kill(true) 結束 node 與其子孫，在
    `-KillWaitMs` 內確認 HasExited、並把輸出讀完（逾時前已寫出的部分照樣回傳）才回 ExitCode=-1；結束不了或讀不完
    （仍有子孫握著管線）丟例外，不留行程。呼叫端只能在 ExitCode=0 時採信 StdOut；StdErr 只供記錄，不得拿來比對結果。
#>
Set-StrictMode -Version Latest

function Invoke-NodeUtf8 {
    param(
        [Parameter(Mandatory)][AllowEmptyString()][string[]]$ArgumentList,
        [int]$TimeoutSec = 120,
        [int]$KillWaitMs = 5000,
        [string]$NodePath = 'node'
    )
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $NodePath
    foreach ($a in $ArgumentList) { $psi.ArgumentList.Add($a) }
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    $psi.StandardOutputEncoding = $utf8
    $psi.StandardErrorEncoding = $utf8
    $p = [System.Diagnostics.Process]::Start($psi)
    $nodePid = $p.Id
    try {
        $outTask = $p.StandardOutput.ReadToEndAsync()
        $errTask = $p.StandardError.ReadToEndAsync()
        $timedOut = -not $p.WaitForExit($TimeoutSec * 1000)
        if ($timedOut) {
            # 逾時：結束 node 與其子孫（Kill(true)），有限時間內確認真的結束；結束不了是硬錯誤（不得回傳後留下行程）。
            $killError = $null
            try { $p.Kill($true) } catch { $killError = $_.Exception.Message }
            if (-not $p.WaitForExit($KillWaitMs) -or -not $p.HasExited) {
                throw "Invoke-NodeUtf8：node（PID $nodePid）逾時 $TimeoutSec 秒後無法結束$(if ($killError) { "：$killError" })"
            }
        } else {
            $p.WaitForExit()   # 無參數版本等到重新導向的輸出讀完
        }
        # 釋放前把輸出讀完；子孫仍握著管線（讀不到 EOF）就是沒結束乾淨，同樣是硬錯誤。
        if (-not [System.Threading.Tasks.Task]::WaitAll([System.Threading.Tasks.Task[]]@($outTask, $errTask), $KillWaitMs)) {
            throw "Invoke-NodeUtf8：node（PID $nodePid）$KillWaitMs ms 內讀不完輸出（仍有子孫行程握著管線？）"
        }
        $out = $outTask.Result.Trim(); $err = $errTask.Result.Trim()
        if ($timedOut) {
            return [PSCustomObject]@{ ExitCode = -1; StdOut = $out; StdErr = (@("node 逾時（$TimeoutSec 秒），已結束", $err) | Where-Object { $_ }) -join "`n"; Pid = $nodePid; TimedOut = $true }
        }
        return [PSCustomObject]@{ ExitCode = $p.ExitCode; StdOut = $out; StdErr = $err; Pid = $nodePid; TimedOut = $false }
    } finally {
        $p.Dispose()
    }
}

Export-ModuleMember -Function Invoke-NodeUtf8
