<#
.SYNOPSIS
    Task 5.1 fix round 1 驗收（Codex [high]：single-instance 同時啟動競態）：同時啟動多個宿主
    行程，重複數十次，驗證永遠只有一個行程存活、只有一組小工具。只讀行程／視窗列舉，
    **不注入任何輸入**（不送按鍵、不點擊、不截圖），鎖定時也可跑。

.DESCRIPTION
    競態（見 tauri-plugin-single-instance 2.5.0 `platform_impl/windows.rs`）：plugin 先
    `CreateMutexW` 再 `CreateWindowExW` 建交接視窗；第二個行程若剛好在兩者之間啟動，mutex 已存在
    但 `FindWindowW` 找不到視窗 → 落到 `Ok(())` 照常啟動 → 兩組小工具。

    為了把「同時」壓到最緊：所有行程先以 `CREATE_SUSPENDED` 建立，再在一個緊密迴圈裡依序
    `ResumeThread`（彼此相差微秒級），比 `Start-Process` 逐一啟動（相差數十毫秒）更容易撞進
    上述視窗。

    每一輪：
      1. 全新暫存 %APPDATA%／%LOCALAPPDATA%（首次啟動；不碰使用者真正的設定檔與資料目錄）。
      2. 同時啟動 N 個宿主（參數輪流帶 `--autostart`／`--restarted`＝spec「開機自啟與系統重新
         啟動同時啟動」；`-Manual` 時第一個不帶參數）。
      3. 等 SettleSec 秒讓落敗者結束、勝出者建立小工具視窗。
      4. 判定：存活的 fc-host 行程數＝1，且擁有可見「fc-host clock」視窗的行程數＝1、該行程
         只有一扇 clock 視窗。
      5. 強制結束全部 fc-host、刪暫存目錄。

    `-Simulate <case>`：不做同時啟動，改以本腳本自己持有具名 mutex 模擬「第一個執行個體卡在
    啟動臨界區」的狀態，決定性地重現／驗證各分支（見 `Invoke-SimulateCase`）。

.PARAMETER Exe
    fc-host.exe 路徑，預設 host/target/release/fc-host.exe。

.PARAMETER Iterations
    重複輪數，預設 30。

.PARAMETER Count
    每輪同時啟動的行程數，預設 2。

.PARAMETER SettleSec
    每輪啟動後等待收斂的秒數，預設 8。

.PARAMETER OutDir
    證據輸出目錄，預設 host/tools/evidence。

.PARAMETER Tag
    記錄檔名後綴（區分 RED／GREEN、不同 Count），預設空字串。
#>
[CmdletBinding()]
param(
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\release\fc-host.exe'),
    [int]$Iterations = 30,
    [int]$Count = 2,
    [int]$SettleSec = 8,
    [switch]$Manual,
    [ValidateSet('', 'plugin-mutex-only', 'startup-held-then-secondary', 'startup-held-then-dead')]
    [string]$Simulate = '',
    [string]$OutDir = (Join-Path $PSScriptRoot 'evidence'),
    [string]$Tag = ''
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'lib\EvidenceLog.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'lib\ProcessTree.psm1') -Force

# 本腳本自己建立的宿主（PID＋啟動時間）；收尾只停這些與其子孫，不以行程名稱停止（fix F4 共通規則）。
$script:StartedHosts = New-Object System.Collections.Generic.List[object]

Add-Type -Namespace V51C -Name Native -MemberDefinition @'
[StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
public struct STARTUPINFO {
    public int cb; public string lpReserved; public string lpDesktop; public string lpTitle;
    public int dwX; public int dwY; public int dwXSize; public int dwYSize;
    public int dwXCountChars; public int dwYCountChars; public int dwFillAttribute; public int dwFlags;
    public short wShowWindow; public short cbReserved2; public System.IntPtr lpReserved2;
    public System.IntPtr hStdInput; public System.IntPtr hStdOutput; public System.IntPtr hStdError;
}
[StructLayout(LayoutKind.Sequential)]
public struct PROCESS_INFORMATION {
    public System.IntPtr hProcess; public System.IntPtr hThread; public int dwProcessId; public int dwThreadId;
}
[DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
public static extern bool CreateProcessW(string app, System.Text.StringBuilder cmd, System.IntPtr pa, System.IntPtr ta,
    bool inherit, uint flags, System.IntPtr env, string cwd, ref STARTUPINFO si, out PROCESS_INFORMATION pi);
[DllImport("kernel32.dll")] public static extern uint ResumeThread(System.IntPtr h);
[DllImport("kernel32.dll")] public static extern bool CloseHandle(System.IntPtr h);
[DllImport("user32.dll")] public static extern System.IntPtr GetTopWindow(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint cmd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
'@

$CREATE_SUSPENDED = 0x4

function Get-Ts { Get-Date -Format 'yyyy-MM-ddTHH:mm:ss.fffK' }
function Test-Locked { [bool](Get-Process -Name LogonUI, LockApp -ErrorAction SilentlyContinue) }

# 所有可見頂層視窗 → @{ pid = @(title, ...) }
function Get-VisibleTitlesByPid {
    $map = @{}
    $h = [V51C.Native]::GetTopWindow([IntPtr]::Zero)
    while ($h -ne [IntPtr]::Zero) {
        if ([V51C.Native]::IsWindowVisible($h)) {
            $p = 0
            [void][V51C.Native]::GetWindowThreadProcessId($h, [ref]$p)
            $sb = New-Object System.Text.StringBuilder 256
            [void][V51C.Native]::GetWindowText($h, $sb, 256)
            if ($sb.Length -gt 0) {
                if (-not $map.ContainsKey([int]$p)) { $map[[int]$p] = @() }
                $map[[int]$p] += $sb.ToString()
            }
        }
        $h = [V51C.Native]::GetWindow($h, 2)
    }
    return $map
}

function New-TempProfile {
    $root = Join-Path ([IO.Path]::GetTempPath()) ("fc-host-5.1c-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
    $roaming = Join-Path $root 'Roaming'
    $local = Join-Path $root 'Local'
    New-Item -ItemType Directory -Force -Path $roaming, $local | Out-Null
    [pscustomobject]@{ Root = $root; Roaming = $roaming; Local = $local }
}

# 以 CREATE_SUSPENDED 建 N 個行程（繼承本行程環境變數，呼叫前已把 APPDATA 指到暫存），再一口氣
# ResumeThread。回傳 PID 陣列。
function Start-Simultaneous([string[][]]$ArgvList, $Prof) {
    $oldA = $env:APPDATA; $oldL = $env:LOCALAPPDATA
    $pis = @()
    try {
        $env:APPDATA = $Prof.Roaming
        $env:LOCALAPPDATA = $Prof.Local
        foreach ($argv in $ArgvList) {
            $cmd = '"' + $script:Exe + '"'
            if ($argv.Count -gt 0) { $cmd += ' ' + ($argv -join ' ') }
            $si = New-Object V51C.Native+STARTUPINFO
            $si.cb = [Runtime.InteropServices.Marshal]::SizeOf($si)
            $pi = New-Object V51C.Native+PROCESS_INFORMATION
            $sb = New-Object System.Text.StringBuilder $cmd
            if (-not [V51C.Native]::CreateProcessW($script:Exe, $sb, [IntPtr]::Zero, [IntPtr]::Zero, $false,
                    $CREATE_SUSPENDED, [IntPtr]::Zero, (Split-Path $script:Exe), [ref]$si, [ref]$pi)) {
                throw "CreateProcessW 失敗：$([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
            }
            $pis += $pi
            # 建立後（仍暫停中）立刻記下，收尾時只停這些。
            $script:StartedHosts.Add([PSCustomObject]@{
                    Id = [int]$pi.dwProcessId
                    StartTime = (Get-ProcessStartTimeOrNull (Get-Process -Id $pi.dwProcessId -ErrorAction SilentlyContinue))
                })
        }
    } finally {
        $env:APPDATA = $oldA; $env:LOCALAPPDATA = $oldL
    }
    foreach ($pi in $pis) { [void][V51C.Native]::ResumeThread($pi.hThread) }
    foreach ($pi in $pis) {
        [void][V51C.Native]::CloseHandle($pi.hThread)
        [void][V51C.Native]::CloseHandle($pi.hProcess)
    }
    return @($pis | ForEach-Object { $_.dwProcessId })
}

# 宿主記錄檔（%LOCALAPPDATA%/tw.fintools.fc-host/logs/，task 5.8）裡的啟動仲裁結果統計；修正前的
# 建置沒有這些記錄行，統計全為 0。
function Get-ArbitrationSummary($Prof) {
    $lines = @(Get-ChildItem -Path (Join-Path $Prof.Local 'tw.fintools.fc-host\logs') -Filter '*.log' -ErrorAction SilentlyContinue |
        ForEach-Object { Get-Content $_.FullName -ErrorAction SilentlyContinue })
    $n = { param($pat) @($lines | Where-Object { $_ -match $pat }).Count }
    "primary=$(& $n '角色=Primary') secondary=$(& $n '角色=Secondary') unarbitrated=$(& $n '角色=Unarbitrated') " +
        "secondaryExitInSetup=$(& $n '未交接') startupTimeout=$(& $n '等不到啟動鎖')"
}

function Stop-AllHosts {
    # 只停本腳本建立的宿主與其子孫（fix F4 共通規則：不以行程名稱停止 fc-host）。
    $mine = @($script:StartedHosts.ToArray())
    foreach ($h in $mine) { [void](Stop-ProcessTree -RootId $h.Id -RootStartTime $h.StartTime) }
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt 10) {
        $alive = @($mine | Where-Object {
                $p = Get-Process -Id $_.Id -ErrorAction SilentlyContinue
                $p -and ($null -eq $_.StartTime -or (Get-ProcessStartTimeOrNull $p) -eq $_.StartTime)
            })
        if ($alive.Count -eq 0) { break }
        Start-Sleep -Milliseconds 200
    }
    $script:StartedHosts.Clear()
}

# 判定一輪結果：存活行程數、擁有 clock 視窗的行程數。
function Measure-Instances([int[]]$Pids) {
    $alive = @($Pids | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })
    $titles = Get-VisibleTitlesByPid
    $clockOwners = @()
    $clockTotal = 0
    foreach ($p in $Pids) {
        if ($titles.ContainsKey($p)) {
            $n = @($titles[$p] | Where-Object { $_ -eq 'fc-host clock' }).Count
            if ($n -gt 0) { $clockOwners += $p; $clockTotal += $n }
        }
    }
    $settings = 0
    foreach ($p in $alive) {
        if ($titles.ContainsKey($p)) { $settings += @($titles[$p] | Where-Object { $_ -eq '財經桌布設定' }).Count }
    }
    [pscustomobject]@{
        Alive = $alive; ClockOwners = $clockOwners; ClockTotal = $clockTotal; SettingsWindows = $settings
        Titles = $titles
    }
}

# ── 前置檢查 ─────────────────────────────────────────────────────────────────────
if (Get-Process -Name fc-host -ErrorAction SilentlyContinue) {
    throw '已有 fc-host 在執行，請先結束（避免與本次暫存實例的行程／視窗列舉互相干擾）。'
}
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$suffix = if ($Tag) { "-$Tag" } else { '' }
$name = if ($Simulate) { "5.1-simulate-$Simulate$suffix" } else { "5.1-concurrent-n$Count$suffix" }
$logPath = Join-Path $OutDir "$name.log"
$log = New-EvidenceWriter $logPath
$log.AutoFlush = $true
$exeInfo = Get-Item $Exe
$log.WriteLine("# verify-5.1-concurrent.ps1 start=$(Get-Ts) exe=$Exe mtime=$($exeInfo.LastWriteTime.ToString('s')) " +
    "iterations=$Iterations count=$Count settle=${SettleSec}s manual=$([int][bool]$Manual) simulate=$Simulate locked=$([int](Test-Locked))")

# ── 模擬模式：自己持有具名 mutex，決定性地重現／驗證各分支 ─────────────────────────────
# plugin 的 mutex 名＝`{identifier}-sim`（crate 原始碼 platform_impl/windows.rs，未開 semver
# feature 故無版本後綴）；宿主自己的啟動仲裁名見 host/src/desktop/instance.rs。
$PluginMutex = 'tw.fintools.fc-host-sim'
$StartupMutex = 'Local\tw.fintools.fc-host.startup'
$InstanceMutex = 'Local\tw.fintools.fc-host.instance'

function Invoke-SimulateCase([string]$Case) {
    $prof = New-TempProfile
    $held = @()
    $pids = @()
    try {
        # 以 initiallyOwned=$true 建立＝本執行緒擁有（模擬首個執行個體的主執行緒持有）。
        $held += [System.Threading.Mutex]::new($true, $PluginMutex)
        if ($Case -ne 'plugin-mutex-only') {
            $held += [System.Threading.Mutex]::new($true, $StartupMutex)
            $held += [System.Threading.Mutex]::new($true, $InstanceMutex)
        }
        $pids = Start-Simultaneous -ArgvList @(, @('--autostart')) -Prof $prof
        $script:log.WriteLine("## $(Get-Ts) [$Case] 已持有 mutex：$(if ($Case -eq 'plugin-mutex-only') { 'plugin' } else { 'plugin+startup+instance' })；啟動 pid=$($pids -join ',')")

        if ($Case -ne 'plugin-mutex-only') {
            # 持有啟動鎖 3 秒：新行程應卡在等待、不建立任何小工具。
            Start-Sleep -Seconds 3
            $m = Measure-Instances $pids
            $script:log.WriteLine("## $(Get-Ts) [$Case] 持有啟動鎖 3 秒後：alive=$($m.Alive.Count) clockOwners=$($m.ClockOwners.Count)")
            $script:results["[$Case] 啟動鎖被持有期間新行程不建立小工具"] = ($m.ClockOwners.Count -eq 0)
            if ($Case -eq 'startup-held-then-secondary') {
                # 模擬首個執行個體「已過臨界區但交接視窗不存在」（例如正在結束）：只放開啟動鎖，
                # 執行個體鎖與 plugin mutex 仍持有 → 新行程應判定為 Secondary 並安全結束。
                $held[1].ReleaseMutex(); $held[1].Dispose()
            } else {
                # 模擬首個執行個體在臨界區內死亡：全部放開（ReleaseMutex＝等同行程結束時的
                # abandoned 交接）→ 新行程應成為唯一執行個體、正常建立小工具。
                foreach ($x in $held) { $x.ReleaseMutex(); $x.Dispose() }
                $held = @()
            }
        }

        Start-Sleep -Seconds $SettleSec
        $m = Measure-Instances $pids
        $script:log.WriteLine("## $(Get-Ts) [$Case] 收斂後：alive=$($m.Alive.Count) clockOwners=$($m.ClockOwners.Count) clockTotal=$($m.ClockTotal) $(Get-ArbitrationSummary $prof)")
        switch ($Case) {
            'plugin-mutex-only' {
                # 修正前：plugin 找不到交接視窗 → 照常啟動（缺陷）。修正後：本行程取得啟動鎖與
                # 執行個體鎖（無人持有）→ Primary → 照常啟動（這個狀態只代表「plugin mutex 的
                # 持有者不是執行個體」，例如交接中的落敗者）。兩版皆會啟動，本案例僅作 RED 證據：
                # 證明 plugin 在「mutex 在、視窗不在」時會照常啟動。
                $script:results["[$Case] 記錄：plugin mutex 在、交接視窗不在時新行程是否照常啟動（alive=$($m.Alive.Count) clockOwners=$($m.ClockOwners.Count)）"] = $true
            }
            'startup-held-then-secondary' {
                $script:results["[$Case] 新行程安全結束、未建立小工具"] = ($m.Alive.Count -eq 0 -and $m.ClockOwners.Count -eq 0)
            }
            'startup-held-then-dead' {
                $script:results["[$Case] 新行程成為唯一執行個體並建立小工具"] = ($m.Alive.Count -eq 1 -and $m.ClockOwners.Count -eq 1)
            }
        }
    } finally {
        foreach ($x in $held) { try { $x.ReleaseMutex() } catch {}; $x.Dispose() }
        Stop-AllHosts
        Remove-Item -Recurse -Force $prof.Root -ErrorAction SilentlyContinue
    }
}

$regSnap = @(Save-FcHostAutostartRegistry)
$log.WriteLine("# $(Get-Ts) 開機自啟登錄快照：$(($regSnap | ForEach-Object { "$($_.Name)@$(Split-Path $_.Key -Leaf) Exists=$($_.Exists)" }) -join '; ')")
$results = [ordered]@{}
try {
    if ($Simulate) {
        Invoke-SimulateCase $Simulate
    } else {
        $pass = 0
        $failRounds = @()
        for ($i = 1; $i -le $Iterations; $i++) {
            $prof = New-TempProfile
            try {
                $argvList = @()
                for ($k = 0; $k -lt $Count; $k++) {
                    if ($Manual -and $k -eq 0) { $argvList += , @() }
                    elseif ($k % 2 -eq 0) { $argvList += , @('--autostart') }
                    else { $argvList += , @('--restarted') }
                }
                $pids = Start-Simultaneous -ArgvList $argvList -Prof $prof
                Start-Sleep -Seconds $SettleSec
                $m = Measure-Instances $pids
                $ok = ($m.Alive.Count -eq 1 -and $m.ClockOwners.Count -eq 1 -and $m.ClockTotal -eq 1)
                $arb = Get-ArbitrationSummary $prof
                if ($ok) { $pass++ } else { $failRounds += $i }
                $log.WriteLine("## $(Get-Ts) round=$i pids=$($pids -join ',') alive=$($m.Alive -join ',') " +
                    "clockOwners=$($m.ClockOwners -join ',') clockTotal=$($m.ClockTotal) settings=$($m.SettingsWindows) " +
                    "$arb verdict=$(if ($ok) { 'PASS' } else { 'FAIL' })")
            } finally {
                Stop-AllHosts
                Remove-Item -Recurse -Force $prof.Root -ErrorAction SilentlyContinue
            }
        }
        $log.WriteLine("# 合計：$pass/$Iterations 輪只有一組小工具；FAIL 輪次：$($failRounds -join ',')")
        $results["同時啟動 $Count 個行程 ×$Iterations 輪，每輪只有一個存活行程、一組小工具（$pass/$Iterations）"] = ($pass -eq $Iterations)
    }
} finally {
    Stop-AllHosts
    $regLeft = @(Restore-FcHostAutostartRegistry $regSnap)
    if ($regLeft.Count -gt 0) { Write-Warning "開機自啟登錄未還原：$($regLeft -join '; ')" }
    $log.WriteLine("# $(Get-Ts) 開機自啟登錄還原：未還原 $($regLeft.Count) 項$(if ($regLeft.Count) { '：' + ($regLeft -join '; ') })")
    $results['開機自啟登錄已還原（Run／StartupApproved 的 fc-host）'] = ($regLeft.Count -eq 0)
    $log.WriteLine("# $(Get-Ts) 結束；殘留 fc-host=$(@(Get-Process -Name fc-host -ErrorAction SilentlyContinue).Count)")
    foreach ($k in $results.Keys) { $log.WriteLine("$(if ($results[$k]) { 'PASS' } else { 'FAIL' })  $k") }
    $log.Close()
}

Get-Content $logPath | Select-Object -Last 6
$failed = @($results.Values | Where-Object { -not $_ })
if ($failed.Count -gt 0) { exit 1 }
exit 0
