<#
.SYNOPSIS
    只停「自己啟動的那個行程」與其子孫行程（fix F4 共通規則）。

.DESCRIPTION
    驗收腳本不得以行程名稱停止 fc-host（`Stop-Process -Name fc-host`、
    `Get-Process fc-host | Stop-Process` 等）：使用者自己的宿主或 24 小時連跑（soak-6.2.ps1）的
    宿主會被一併殺掉（review B-batch3 high、B-batch2 low）。一律以 `Start-Process -PassThru`
    取得的 PID 為根，只停它與其子孫（WebView2 的 msedgewebview2 等）。

    - `Get-ProcessTreeIds -RootId <pid> [-RootStartTime <時間>] -Processes <清單>`：純函式。
      清單元素需有 ProcessId、ParentProcessId，可選 CreationDate。回傳根與所有子孫的 PID
      （根在最前）。PID 重用防護（有建立時間才做）：
        - 子行程建立時間早於父行程＝父 PID 是被重用的，不列入；
        - 給了 RootStartTime 而清單裡同 PID 行程的建立時間與它相差超過 2 秒＝根已結束、
          PID 被別的行程重用：不列入根本身，也只收建立時間落在「原本的根啟動之後、重用者
          啟動之前」的直接子行程（原本的根留下的孤兒）。
      -RootExited（呼叫端確知根已結束）：根本身一律不收，孤兒規則同上。
    - `Stop-ProcessTree -Process <Start-Process 物件>`（或 `-RootId <pid> [-RootStartTime <時間>]`）：先一次取得 Win32_Process 快照、
      算出整棵樹，再逐一 Stop-Process -Id（先停根，避免它再生子行程）。回傳嘗試停止的 PID。
      根已不存在時仍會清它留下的子孫（ParentProcessId 仍指向根 PID）。
    - `Get-ProcessStartTimeOrNull <Process>`：取 StartTime，取不到回傳 $null（行程已結束等）。
#>
Set-StrictMode -Version Latest

function Get-ProcessStartTimeOrNull($Process) {
    if ($null -eq $Process) { return $null }
    try { return [datetime]$Process.StartTime } catch { return $null }
}

function Get-ProcessTreeIds {
    param([Parameter(Mandatory)][int]$RootId, $RootStartTime = $null, [object[]]$Processes = @(), [switch]$RootExited)
    $byParent = @{}
    $created = @{}
    foreach ($p in $Processes) {
        $procId = [int]$p.ProcessId
        $parentId = [int]$p.ParentProcessId
        $hasCreation = $p.PSObject.Properties.Name -contains 'CreationDate'
        if ($hasCreation -and $p.CreationDate) { $created[$procId] = [datetime]$p.CreationDate }
        if ($procId -eq $parentId) { continue }
        if (-not $byParent.ContainsKey($parentId)) { $byParent[$parentId] = New-Object System.Collections.Generic.List[object] }
        $byParent[$parentId].Add($p)
    }
    $rootReused = $false
    $reusedAt = $null
    if ($RootExited) {
        # 根已結束：清單裡若還有同 PID 的行程，一定是重用者。
        $rootReused = $true
        if ($created.ContainsKey($RootId)) { $reusedAt = $created[$RootId] }
    } elseif ($null -ne $RootStartTime -and $created.ContainsKey($RootId)) {
        if ([Math]::Abs(($created[$RootId] - [datetime]$RootStartTime).TotalSeconds) -gt 2) {
            $rootReused = $true
            $reusedAt = $created[$RootId]
        }
    }
    $rootStart = if ($null -ne $RootStartTime) { [datetime]$RootStartTime } elseif (-not $rootReused -and $created.ContainsKey($RootId)) { $created[$RootId] } else { $null }

    $result = New-Object System.Collections.Generic.List[int]
    $seen = New-Object System.Collections.Generic.HashSet[int]
    $queue = New-Object System.Collections.Generic.Queue[int]
    [void]$seen.Add($RootId)
    if (-not $rootReused) { $result.Add($RootId) }
    $queue.Enqueue($RootId)
    while ($queue.Count -gt 0) {
        $cur = $queue.Dequeue()
        if (-not $byParent.ContainsKey($cur)) { continue }
        foreach ($child in $byParent[$cur]) {
            $childId = [int]$child.ProcessId
            if ($seen.Contains($childId)) { continue }
            $childCreated = if ($created.ContainsKey($childId)) { $created[$childId] } else { $null }
            if ($cur -eq $RootId) {
                if ($null -ne $childCreated -and $null -ne $rootStart -and $childCreated -lt $rootStart) { continue }
                if ($rootReused) {
                    # 根已結束／被重用：只收「確定是原本的根留下的」孤兒——建立時間已知、不早於原本
                    # 的根啟動（啟動時間未知就不收）、早於重用者啟動。
                    if ($null -eq $childCreated -or $null -eq $rootStart) { continue }
                    if ($null -ne $reusedAt -and $childCreated -ge $reusedAt) { continue }
                }
            } elseif ($null -ne $childCreated -and $created.ContainsKey($cur) -and $childCreated -lt $created[$cur]) {
                continue
            }
            [void]$seen.Add($childId)
            $result.Add($childId)
            $queue.Enqueue($childId)
        }
    }
    return $result.ToArray()
}

function Stop-ProcessTree {
    <#
    -Process：Start-Process -PassThru 取得的物件（建議）；會用它的 Id、StartTime 與 HasExited
    （物件持有行程控制代碼，PID 被重用時 HasExited 仍正確）。或改給 -RootId［-RootStartTime］。
    #>
    param([int]$RootId = 0, $RootStartTime = $null, [System.Diagnostics.Process]$Process = $null)
    $exited = $false
    if ($Process) {
        $RootId = $Process.Id
        if ($null -eq $RootStartTime) { $RootStartTime = Get-ProcessStartTimeOrNull $Process }
        try { $exited = [bool]$Process.HasExited } catch { $exited = $false }
    }
    if ($RootId -le 0) { throw 'Stop-ProcessTree：需要 -Process 或 -RootId' }
    $all = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
            Select-Object ProcessId, ParentProcessId, CreationDate)
    $ids = @(Get-ProcessTreeIds -RootId $RootId -RootStartTime $RootStartTime -Processes $all -RootExited:$exited)
    foreach ($procId in $ids) { Stop-Process -Id $procId -Force -ErrorAction SilentlyContinue }
    return $ids
}

Export-ModuleMember -Function Get-ProcessTreeIds, Stop-ProcessTree, Get-ProcessStartTimeOrNull
