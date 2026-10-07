<#
.SYNOPSIS
    啟動 fc-host 的驗收腳本共用：開機自啟相關登錄值的快照／還原（fix F2，review 5.4）。

.DESCRIPTION
    宿主在使用者切換設定視窗的「登入時自動啟動」時會寫入／刪除
    HKCU\Software\Microsoft\Windows\CurrentVersion\Run\fc-host（值內容是這次執行的 exe 路徑；
    2026-10-01 起首次啟動不再寫，初次登錄歸安裝檔）。HKCU 不受暫存 %APPDATA% 覆寫影響，舊版
    宿主（首次啟動會寫）與會切換開關的驗收都會改到真正的登錄，所以每支會啟動宿主
    的腳本都必須在啟動前 Save-FcHostAutostartRegistry、結束時（宿主已停止後）
    Restore-FcHostAutostartRegistry，把下面兩個值還原成腳本開始前的樣子（含「原本不存在」）：

      - HKCU\Software\Microsoft\Windows\CurrentVersion\Run 的 fc-host
      - HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run 的 fc-host
        （宿主自 fix F2 起不再寫它；舊版／tauri-plugin-autostart 會寫，一併還原以防萬一）

    讀寫經可替換的後端（Set-FcHostRegistryBackend），測試以記憶體內的假後端驗證，不碰真正的
    登錄（host/tools/tests/AutostartRegistry.Tests.ps1）。

.EXAMPLE
    Import-Module (Join-Path $PSScriptRoot 'lib\AutostartRegistry.psm1') -Force
    $regSnap = Save-FcHostAutostartRegistry
    try { ...啟動宿主、驗收... }
    finally {
        ...停止宿主...
        $notRestored = Restore-FcHostAutostartRegistry $regSnap
        if ($notRestored) { Write-Warning "未還原：$($notRestored -join '; ')" }
    }
#>
Set-StrictMode -Version Latest

$script:Targets = @(
    [PSCustomObject]@{ Key = 'Software\Microsoft\Windows\CurrentVersion\Run'; Name = 'fc-host' }
    [PSCustomObject]@{ Key = 'Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run'; Name = 'fc-host' }
)

# ── 真正的後端（HKCU；只在驗收腳本實際執行時被呼叫）────────────────────────────────
$script:RealBackend = @{
    # 回傳 $null（不存在）或 @{ Kind = [Microsoft.Win32.RegistryValueKind]; Value = <原值> }
    Get    = {
        param([string]$Key, [string]$Name)
        $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($Key, $false)
        if (-not $k) { return $null }
        try {
            if (-not ($k.GetValueNames() -contains $Name)) { return $null }
            return @{
                Kind  = $k.GetValueKind($Name)
                Value = $k.GetValue($Name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            }
        } finally { $k.Close() }
    }
    Set    = {
        param([string]$Key, [string]$Name, $Kind, $Value)
        $k = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($Key)
        try { $k.SetValue($Name, $Value, $Kind) } finally { $k.Close() }
    }
    Remove = {
        param([string]$Key, [string]$Name)
        $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($Key, $true)
        if (-not $k) { return }
        try { $k.DeleteValue($Name, $false) } finally { $k.Close() }
    }
}
$script:Backend = $script:RealBackend

function Set-FcHostRegistryBackend {
    <# 測試用：換掉讀寫後端（Get／Set／Remove 三個 scriptblock）；-Real 換回真正的登錄。 #>
    param([hashtable]$Backend, [switch]$Real)
    if ($Real) { $script:Backend = $script:RealBackend } else { $script:Backend = $Backend }
}

function Save-FcHostAutostartRegistry {
    <#
    讀取兩個目標值，回傳快照（每項 Key／Name／Exists／Kind／Value）。讀取失敗直接丟例外——
    沒有快照就不該啟動宿主（之後無從還原）。
    #>
    foreach ($t in $script:Targets) {
        $v = & $script:Backend.Get $t.Key $t.Name
        # 不用 `$(if …)` 取值：子運算式會把 byte[] 攤平成 object[]，Binary 值寫回時
        # RegistryKey.SetValue 會丟型別不符例外（StartupApproved 還原失敗）。
        $kind = $null
        $value = $null
        if ($v) { $kind = $v.Kind; $value = $v.Value }
        [PSCustomObject]@{
            Key    = $t.Key
            Name   = $t.Name
            Exists = [bool]$v
            Kind   = $kind
            Value  = $value
        }
    }
}

function Restore-FcHostAutostartRegistry {
    <#
    把快照裡的每一項還原（原本存在＝寫回原型別與原值；原本不存在＝刪除）。逐項 try/catch，
    任何一項失敗都不中斷其餘項目；回傳未還原項目的描述字串陣列（空＝全部還原）。
    #>
    param([Parameter(Mandatory)][AllowEmptyCollection()][object[]]$Snapshot)
    $failed = New-Object System.Collections.Generic.List[string]
    foreach ($item in $Snapshot) {
        $where = "HKCU\$($item.Key)\$($item.Name)"
        try {
            if ($item.Exists) {
                & $script:Backend.Set $item.Key $item.Name $item.Kind $item.Value
            } else {
                & $script:Backend.Remove $item.Key $item.Name
            }
        } catch {
            $failed.Add("$where（$(if ($item.Exists) { "應還原為 $($item.Kind)" } else { '應刪除' })）：$($_.Exception.Message)")
        }
    }
    # 不加前置逗號：呼叫端一律以 @(...) 或 [string[]](...) 收集，前置逗號會讓空清單被包成
    # 「含一個空陣列的陣列」（Count＝1），每支腳本都誤判「未還原：System.String[]」（批次 A 實測）。
    return $failed.ToArray()
}

Export-ModuleMember -Function Set-FcHostRegistryBackend, Save-FcHostAutostartRegistry, Restore-FcHostAutostartRegistry
