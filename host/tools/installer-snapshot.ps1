<#
.SYNOPSIS
    安裝／更新／解除安裝實機測試前後的「快照、還原、比對」（installer-auto-update task 4.1、design.md D7）。

.DESCRIPTION
    保護使用者真實設定檔：實機測試會安裝、更新、解除安裝 fc-host，這些動作會改動下列位置；測試前先 Save，
    測完 Restore（結束時自動 Compare），任何殘留差異都會列出來。

    受管範圍（路徑與登錄鍵全部寫死在 lib\InstallerSnapshot.psm1 的 New-FcRealTargets，命令列不接受任何路徑參數，
    只收快照目錄；資料夾位置取自 known folder，環境變數 LOCALAPPDATA／APPDATA 與它不一致就拒絕）：

      目錄（整個複製；不存在就記「不存在」，還原時確保不存在）
        %LOCALAPPDATA%\fc-host                    安裝目錄（installer.nsi 的 INSTALLDIR：
                                                  `$LOCALAPPDATA\${PRODUCTNAME}`，rendered installer.nsi:506）
        %APPDATA%\tw.fintools.fc-host             設定與桌布狀態（identifier；installer.nsi:834 RmDir /r "$APPDATA\${BUNDLEID}"）
        %LOCALAPPDATA%\tw.fintools.fc-host        資料與記錄（installer.nsi:835）
      檔案
        <Programs>\fc-host.lnk                    範本建立的開始功能表捷徑（installer.nsi:903；Programs＝known folder，
                                                  預設 %APPDATA%\Microsoft\Windows\Start Menu\Programs）
        <Programs>\財經日曆.lnk                   hooks.nsh POSTINSTALL 改名後的捷徑
        <Desktop>\fc-host.lnk                     範本建立的桌面捷徑（/S、passive 在安裝區段、互動在完成頁勾選時；
                                                  installer.nsi:927；解除安裝只在目標吻合時才刪，所以要快照）
        <Desktop>\財經日曆.lnk                    hooks.nsh POSTINSTALL／.onGUIEnd 改名後的桌面捷徑（解除安裝同樣
                                                  只在目標吻合時才刪）
      登錄（HKCU；單值＝不存在與空字串分開記）
        Software\Microsoft\Windows\CurrentVersion\Run                         值 fc-host
        Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run 值 fc-host
        Software\Microsoft\Windows\CurrentVersion\Uninstall\fc-host           整棵子樹（UNINSTKEY＝
                                                                              "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}"，
                                                                              rendered installer.nsi:62；原始範本 :67，見 hooks.nsh:211）
        Software\fintools\fc-host                                             整棵子樹（MANUPRODUCTKEY＝"${MANUKEY}\${PRODUCTNAME}"，
                                                                              MANUKEY＝"Software\${MANUFACTURER}"，MANUFACTURER＝"fintools"，
                                                                              rendered installer.nsi:36、63、64）；父鍵 Software\fintools
                                                                              快照時不存在、還原後為空鍵才刪（對應 :826 DeleteRegKey /ifempty）
        Control Panel\Desktop                                                 值 Wallpaper／WallpaperStyle／TileWallpaper
      桌布讀回（只比對、絕不設定；設桌布歸宿主的 --restore-wallpaper）
        IDesktopWallpaper：各螢幕 GetWallpaper、全域 GetPosition／GetBackgroundColor／GetStatus
      僅回報（只做 Save 與 Compare，絕不還原；explorer 的桌布轉存工作檔，差異不算失敗）
        %APPDATA%\Microsoft\Windows\Themes\ 下的 TranscodedWallpaper、Transcoded_*、TranscodedWallpaperCache\*

    「rendered installer.nsi」＝ host/target/release/nsis/<arch>/installer.nsi（bundler 渲染結果，gitignore；
    行號與原始範本 tauri-cli-v2.12.1 不同，以 `grep -n UNINSTKEY` 等對帳）。

    安全：
      - Save／Restore 前檢查沒有 fc-host、fc-host 的 msedgewebview2、安裝檔、安裝目錄內的 uninstall.exe 在跑，
        有就停下（結束碼 3），本腳本不會結束任何行程。
      - Save 結束時自動對現況 Compare：不是零差異就把快照標成不完整（結束碼 1，不可用於還原）。
      - Save 不覆寫既有快照（目錄必須不存在或為空）；快照目錄不得位於任何會被還原刪除的目錄之內。
      - 受管目錄內若有 reparse point（junction／symlink），Save 拒絕。
      - Restore 在任何刪除之前先確認快照內容完整（每個「存在」的項目都有對應內容），缺一項整個拒絕（結束碼 3）。
      - Restore 只刪上列寫死的目錄、登錄鍵與檔案；刪除前擋空值、磁碟根目錄、過淺路徑與 reparse point。
      - 快照記錄的路徑必須與目前寫死的路徑一致，否則拒絕（防止拿別人的快照來還原）。
      - -WhatIf：Save／Restore 只列出將執行的動作，不改任何東西。

.PARAMETER Command
    Save｜Restore｜Compare。Restore 結束時自動跑 Compare。

.PARAMETER Dir
    快照目錄（Save 時必須不存在或為空）。

.PARAMETER WhatIf
    只列出動作（Save、Restore 有效；Compare 本來就只讀）。

.EXAMPLE
    pwsh -NoProfile -File host/tools/installer-snapshot.ps1 Save -Dir D:\snap\run1
    pwsh -NoProfile -File host/tools/installer-snapshot.ps1 Restore -Dir D:\snap\run1 -WhatIf
    pwsh -NoProfile -File host/tools/installer-snapshot.ps1 Restore -Dir D:\snap\run1
    pwsh -NoProfile -File host/tools/installer-snapshot.ps1 Compare -Dir D:\snap\run1

    結束碼：0＝成功且零差異（僅回報項的差異不影響）；1＝有差異、還原動作失敗或 Save 自我驗證失敗
    （參數綁定錯誤也由 pwsh 回 1，且在任何動作之前）；3＝拒絕（行程在跑、快照不合法或不完整、環境變數與
    known folder 不一致等，未動任何東西）；4＝非預期錯誤。
#>
#Requires -Version 7.5
param(
    [Parameter(Mandatory, Position = 0)][ValidateSet('Save', 'Restore', 'Compare')][string]$Command,
    [Parameter(Mandatory)][string]$Dir,
    [switch]$WhatIf
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

Import-Module (Join-Path $PSScriptRoot 'lib\InstallerSnapshot.psm1') -Force

function Write-Summary($Differences, $ReportOnly, $Warnings, $WallpaperVerified) {
    foreach ($w in $Warnings) { Write-Warning $w }
    if (-not $WallpaperVerified) { Write-Host '注意：桌布未驗證（讀回失敗或未比對），零差異結論不涵蓋桌布' -ForegroundColor Yellow }
    if ($ReportOnly.Count -gt 0) {
        Write-Host "僅回報（不還原、不影響結束碼）：$($ReportOnly.Count) 項" -ForegroundColor Cyan
        $ReportOnly | ForEach-Object { Write-Host "  ~ $_" }
    }
}

try {
    $targets = New-FcRealTargets (Resolve-FcProfilePaths)
    $ctx = New-FcSnapshotContext
    switch ($Command) {
        'Save' {
            $r = Save-FcSnapshot -Dir $Dir -Targets $targets -Ctx $ctx -WhatIf:$WhatIf
            if ($WhatIf) { exit 0 }
            Write-Summary $r.Differences $r.ReportOnly $r.Warnings $true
            if ($r.Complete) { exit 0 }
            exit 1
        }
        'Compare' {
            $r = Compare-FcSnapshot -Dir $Dir -Targets $targets -Ctx $ctx
            Write-Summary $r.Differences $r.ReportOnly $r.Warnings $r.WallpaperVerified
            if ($r.Differences.Count -eq 0) { Write-Host '比對結果：零差異'; exit 0 }
            Write-Host "比對結果：$($r.Differences.Count) 項差異" -ForegroundColor Yellow
            $r.Differences | ForEach-Object { Write-Host "  - $_" }
            exit 1
        }
        'Restore' {
            $r = Restore-FcSnapshot -Dir $Dir -Targets $targets -Ctx $ctx -WhatIf:$WhatIf
            if ($WhatIf) { exit 0 }
            Write-Summary $r.Differences $r.ReportOnly $r.Warnings $r.WallpaperVerified
            foreach ($e in $r.Errors) { Write-Host "還原失敗項：$e" -ForegroundColor Red }
            if ($r.Differences.Count -eq 0 -and $r.Errors.Count -eq 0) { Write-Host '還原完成：比對零差異'; exit 0 }
            Write-Host "還原後仍有 $($r.Differences.Count) 項差異、$($r.Errors.Count) 項動作失敗" -ForegroundColor Yellow
            $r.Differences | ForEach-Object { Write-Host "  - $_" }
            exit 1
        }
    }
} catch [System.InvalidOperationException] {
    Write-Host $_.Exception.Message -ForegroundColor Red
    exit 3
} catch {
    Write-Host "非預期錯誤：$($_.Exception.Message)" -ForegroundColor Red
    exit 4
}
