; fc-host NSIS 安裝檔 hook（installer-auto-update task 2.1、design.md D2）。
;
; 本檔**要求存成 UTF-8 含 BOM**（防禦性要求）：目前 bundler 以 `-INPUTCHARSET UTF8` 呼叫 makensis
; （tauri-bundler 2.12.1 nsis/mod.rs），沒有 BOM 也能讀對；但 bundler 只替它自己產生的主腳本加 BOM，
; 日後若改變呼叫方式，沒有 BOM 的 include 檔中文會被當 ANSI 讀成亂碼。package.ps1 打包前檢查開頭是
; EF BB BF，不是就失敗。
;
; 行號引用的是 tauri-cli-v2.12.1 的原始範本（tag 固定）：
;   installer.nsi = crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi
;   utils.nsh     = 同目錄 utils.nsh
; 注意：bundler 渲染後的 target/release/nsis/<arch>/installer.nsi 會把 handlebars 區塊行（{{#if}} 等）
; 拿掉，行號與原始範本不同，以原始範本為準。
;
; 這些 hook 在「手動覆蓋安裝」時由**舊版**的解除安裝程式執行（installer.nsi:356-361 PageLeaveReinstall
; 以已安裝的 uninstall.exe 跑非更新模式的解除安裝），所以 v0.1.0 的 hook 寫錯，之後的版本修不回來。
;
; 範本的 NSIS_HOOK_* 巨集插入點（`!ifmacrodef`＋`!insertmacro`，名稱必須完全一致）：
;   PREINSTALL    installer.nsi:642-644（Section Install 內，SetOutPath 之後、CheckIfAppIsRunning 與
;                 複製檔案之前）
;   POSTINSTALL   installer.nsi:734-736（所有登錄值與捷徑都寫完之後）
;   PREUNINSTALL  installer.nsi:779-781（Section Uninstall 的第一件事，早於 CheckIfAppIsRunning 與刪檔）
;   POSTUNINSTALL installer.nsi:887-889（範本刪完捷徑、Run 值、應用程式資料之後）
; 另外本檔自己定義 NSIS 回呼 .onGUIEnd（範本沒有定義）：互動安裝的桌面捷徑在完成頁才建立
; （installer.nsi:411-413），晚於所有 hook，只能在安裝視窗關閉後改名（見 FcRenameDesktopShortcut）。
;
; 「更新模式」＝命令列帶 /UPDATE（外掛的 passive 更新會帶）：安裝程式在 .onInit 設 $UpdateMode
; （installer.nsi:489-492），解除安裝程式在 un.onInit 設（:771-774）。更新模式下 PageLeaveReinstall 直接
; 跳過解除安裝（:319-322），所以更新時舊版解除安裝程式與其 hook 根本不執行；下面的 $UpdateMode 判斷
; 是為了「/UPDATE 直接呼叫解除安裝程式」這類路徑仍然安全。
;
; 暫存器規則：hook 都在區段的固定位置，範本在這些位置之後不依賴 $0-$9／$R0-$R9 的舊值
; （PREINSTALL 之後的 CheckIfAppIsRunning 自己重設 $R0-$R3，utils.nsh:29-35；其餘 hook 之後只剩
; SetAutoClose，installer.nsi:738-741、891-895），故 hook 可以自由使用暫存器；本檔自己的狀態一律用
; 具名變數（Var），不依賴暫存器。

!include "LogicLib.nsh"

; ------------------------------------------------------------------------------------------------
; 可覆寫常數（只供 package.Tests.ps1 的 NSIS 測試框架：縮短等待、把記錄／捷徑／登錄改到暫存位置，
; 讓 hook 能在不碰真實使用者資料的前提下被執行測試；正式建置不定義任何覆寫，一律走下面的預設值）
; ------------------------------------------------------------------------------------------------
; 開始功能表「程式集」資料夾：範本的捷徑建在 $SMPROGRAMS（installer.nsi:951-953，currentUser 時是使用者的）。
!ifndef FC_SM_DIR
  !define FC_SM_DIR "$SMPROGRAMS"
!endif
; 桌面：範本的桌面捷徑建在 $DESKTOP（installer.nsi:976，currentUser 時是使用者的桌面）。
!ifndef FC_DESKTOP_DIR
  !define FC_DESKTOP_DIR "$DESKTOP"
!endif
; 開機自啟 Run 鍵與 StartupApproved\Run 鍵（HKCU；範本刪 Run 值用同一個鍵，installer.nsi:866）。
!ifndef FC_RUN_KEY
  !define FC_RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
!endif
!ifndef FC_STARTUP_APPROVED_KEY
  !define FC_STARTUP_APPROVED_KEY "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"
!endif
; 結束碼 3（宿主沒回報、可能仍在處理）等 15 秒再重跑一次；結束碼 4（宿主還沒準備好或收不到）等 5 秒。
; 數值來源：host/src/wallpaper_cli.rs 模組文件「給安裝檔：結束碼」、design.md D2 PREUNINSTALL。
!ifndef FC_WAIT_CODE3_MS
  !define FC_WAIT_CODE3_MS 15000
!endif
!ifndef FC_WAIT_CODE4_MS
  !define FC_WAIT_CODE4_MS 5000
!endif
; 結束碼 1（還原失敗或狀態檔暫時讀不到）等 5 秒再重跑一次。1 常是暫時性的：顯示器拓樸切換途中、螢幕剛插回
; 而 explorer 還沒跟上、explorer 重啟中，IDesktopWallpaper 讀不到任何在線螢幕，還原前的讀取就整批失敗
; （wallpaper_state.rs「還原前讀取目前桌布失敗，本次不還原」）。立即重跑多半撞同一個暫時錯誤；這類狀況
; 通常數秒內平息，5 秒與結束碼 4 的「剛啟動等幾秒」同量級。這段等待在兩次呼叫之間，每次呼叫本身仍由
; ExecWait 等到它自己結束（不設逾時、不強制終止，符合 INSTALLER_WAIT_RECOMMENDED 120 秒以上的契約），
; 所以只是讓最壞情況多 5 秒，不影響「等到結束」的語意。
!ifndef FC_WAIT_CODE1_MS
  !define FC_WAIT_CODE1_MS 5000
!endif

Var FcHadInstall   ; PREINSTALL 記下：安裝前 $INSTDIR 是否已有主程式（1／0）
Var FcExe          ; 宿主主程式完整路徑（PREUNINSTALL 設定）
Var FcLogDir       ; uninstall.log 所在目錄（PREUNINSTALL 設定）
Var FcShowUi       ; 1＝允許跳訊息框（非 silent 也非 passive）
Var FcWait3Ms      ; 結束碼 3 的重跑前等待（毫秒）
Var FcWait4Ms      ; 結束碼 4 的重跑前等待（毫秒）
Var FcWait1Ms      ; 結束碼 1 的重跑前等待（毫秒）
Var FcWaitMs       ; 本次重跑前實際等待（毫秒）
Var FcAttempt      ; 第幾次嘗試（1 起算）
Var FcCode         ; --restore-wallpaper 的結束碼（字串；ExecWait 起不來時是 "error"）
Var FcMsg          ; 要寫進記錄的一行
Var FcFile         ; 記錄檔 handle
Var FcInstallDone  ; POSTINSTALL 跑完設 1；.onGUIEnd 只在它為 1 時處理桌面捷徑（取消安裝時不動）
Var FcDeskSrc      ; 範本建立的桌面捷徑完整路徑（$DESKTOP\${PRODUCTNAME}.lnk，POSTINSTALL 設定）
Var FcDeskDst      ; 改名後的桌面捷徑完整路徑（$DESKTOP\財經日曆.lnk，POSTINSTALL 設定）
Var FcDeskExe      ; 捷徑應指向的主程式完整路徑（POSTINSTALL 設定）

; ------------------------------------------------------------------------------------------------
; 解除安裝用函式（un. 前綴＝只能在解除安裝程式內呼叫）
;
; 函式本體在 !include 當下就被編譯，此時範本的 !define（BUNDLEID、MAINBINARYNAME、PRODUCTNAME，
; installer.nsi:41-72）與 Var（$PassiveMode、$UpdateMode，:74-78）都還沒宣告，所以函式只用本檔
; 自己的 Var；需要範本值的地方（路徑、模式判斷）在下面的 PREUNINSTALL 巨集裡先填進這些 Var。
; ------------------------------------------------------------------------------------------------

; 附加一行到 uninstall.log：「yyyy-MM-dd HH:mm:ss <$FcMsg>」。
; 記錄只用 ASCII：Unicode NSIS 的 FileWrite 以 ANSI 字碼頁寫入，非 ASCII 字元在其他字碼頁的系統上
; 會變成 ?；記錄寫不出來（目錄建不起來、檔案被鎖）一律吞掉，不能讓記錄影響解除安裝。
Function un.FcLogLine
  Push $0
  Push $1
  Push $2
  Push $3
  Push $4
  Push $5
  Push $6
  ClearErrors
  CreateDirectory "$FcLogDir" ; NSIS 的 CreateDirectory 會遞迴建立中間目錄；已存在不算錯
  ClearErrors ; CreateDirectory 若設了錯誤旗標，不能讓下面的 FileOpen 成功卻被當成失敗
  ; 模式 a 只是「開啟或建立」，檔案指標仍在開頭（不 FileSeek 的話每次都從頭覆寫舊內容、不截短，
  ; 留下殘字）；所以開檔後必須自己 FileSeek 到結尾。package.Tests.ps1 的「重複執行」情境守住這點。
  FileOpen $FcFile "$FcLogDir\uninstall.log" a
  ${IfNot} ${Errors}
    FileSeek $FcFile 0 END
    ; FileFunc 的 GetTime 以「人工函式」按需產生，un. 版名稱＝${un.GetTime}（FileFunc.nsh）；
    ; 輸出依序：日、月、年、星期名、時、分、秒
    ${un.GetTime} "" "L" $0 $1 $2 $3 $4 $5 $6
    FileWrite $FcFile "$2-$1-$0 $4:$5:$6 $FcMsg$\r$\n"
    FileClose $FcFile
  ${EndIf}
  ClearErrors
  Pop $6
  Pop $5
  Pop $4
  Pop $3
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

; 呼叫 `fc-host.exe --restore-wallpaper` 還原桌布，依結束碼處置（host/src/wallpaper_cli.rs 模組文件）。
;
; - `ExecWait` 沒有逾時參數，正好符合契約：指令本身有 90 秒硬上限、一定會自己結束，安裝檔等它即可。
;   **絕不強制終止它**（沒有 TerminateProcess、沒有 taskkill）：還原做到一半被砍，狀態檔留下「還原進行中」
;   標記，而解除安裝後不會再有下一次執行來續做。
; - 0、6 繼續（6＝在線螢幕已還原、離線螢幕留待之後，契約明訂安裝檔可視為完成）。
; - 3 等 FcWait3Ms 再重跑一次；4 等 FcWait4Ms 再重跑一次；1 等 FcWait1Ms 再重跑一次（暫時性讀取失敗，
;   理由見檔頭 FC_WAIT_CODE1_MS）；7 直接重跑一次（已用滿 90 秒總時限，不必再等）。契約對 2、5 與其他碼
;   也是「可重跑一次」，所以同樣重跑一次（不等待：2＝狀態檔讀不懂、5＝內部錯誤、"error"＝起不來，等待
;   改變不了結果）。只重跑一次：第二次仍失敗就放棄，不無限重試。
; - 仍失敗：非 silent、非 passive 以訊息框告知；silent／passive 只寫記錄。然後一律繼續解除安裝。
; - 起不來時 ExecWait 的輸出變數可能是 "error" 或保持原值（實測為後者），所以呼叫前先設成 "error"，
;   之後一律用字串比較（== 是不分大小寫的字串比較），不用 `=` 的數值比較（"error" 會被當成 0）。
Function un.FcRestoreWallpaper
  StrCpy $FcMsg "uninstall hook start: restoring wallpaper before removal"
  Call un.FcLogLine
  StrCpy $FcAttempt 0

  fc_next_attempt:
  IntOp $FcAttempt $FcAttempt + 1
  ${IfNot} ${FileExists} "$FcExe"
    ; 主程式不在了（使用者自己刪過、安裝壞了）：沒有東西可呼叫，也沒有「失敗」可告知
    StrCpy $FcMsg "attempt $FcAttempt: main executable not found; nothing to restore, continuing"
    Call un.FcLogLine
    Return
  ${EndIf}

  StrCpy $FcMsg "attempt $FcAttempt: running --restore-wallpaper (waits for it to exit, never killed)"
  Call un.FcLogLine
  DetailPrint "正在還原桌布…（最長約 90 秒）"
  ClearErrors
  ; 先設成 "error"：實測主程式起不來（檔案不是有效的 Win32 程式）時，ExecWait 不會動輸出變數、保持原值
  ; （空字串），不是文件所說的 "error"；預設成 "error" 才不會被當成成功碼
  StrCpy $FcCode "error"
  ExecWait '"$FcExe" --restore-wallpaper' $FcCode
  StrCpy $FcMsg "attempt $FcAttempt: exit code $FcCode"
  Call un.FcLogLine

  ${If} $FcCode == "0"
    StrCpy $FcMsg "result: wallpaper restored (or nothing to restore); continuing"
    Call un.FcLogLine
    Return
  ${EndIf}
  ${If} $FcCode == "6"
    StrCpy $FcMsg "result: online monitors restored; offline monitors still pending, treated as done per contract; continuing"
    Call un.FcLogLine
    Return
  ${EndIf}

  ${If} $FcAttempt = 1
    ${If} $FcCode == "3"
      StrCpy $FcWaitMs $FcWait3Ms
    ${ElseIf} $FcCode == "4"
      StrCpy $FcWaitMs $FcWait4Ms
    ${ElseIf} $FcCode == "1"
      StrCpy $FcWaitMs $FcWait1Ms ; 暫時性讀取失敗：等平息再重跑（見檔頭 FC_WAIT_CODE1_MS）
    ${Else}
      StrCpy $FcWaitMs 0 ; 7、2、5、其他、"error"：直接重跑一次
    ${EndIf}
    StrCpy $FcMsg "action: retry once after $FcWaitMs ms"
    Call un.FcLogLine
    ${If} $FcWaitMs <> 0
      DetailPrint "等待財經日曆回應後重試還原桌布…"
      Sleep $FcWaitMs
    ${EndIf}
    Goto fc_next_attempt
  ${EndIf}

  ; 第二次仍失敗：放棄，繼續解除安裝
  StrCpy $FcMsg "result: still failing after retry (exit code $FcCode); giving up, wallpaper may stay on the fc-host image; continuing"
  Call un.FcLogLine
  ${If} $FcShowUi = 1
    MessageBox MB_OK|MB_ICONEXCLAMATION "桌布可能停在財經日曆的圖，請到 Windows 設定自行更換。" /SD IDOK
  ${EndIf}
FunctionEnd

; ------------------------------------------------------------------------------------------------
; 安裝用函式：桌面捷徑改名（fc-host.lnk → 財經日曆.lnk）
;
; 範本建立桌面捷徑的時機有兩個，都由 CreateOrUpdateDesktopShortcut（installer.nsi:957-978）在
; $DESKTOP\${PRODUCTNAME}.lnk 建立（:976）：
;   - 被動（/P）與靜默（/S）：Section Install 內 :729-731 呼叫，早於 POSTINSTALL（:734-736）；
;   - 互動：完成頁的「建立桌面捷徑」勾選框（MUI_FINISHPAGE_SHOWREADME_FUNCTION，:411-413），在使用者按
;     「完成」離開完成頁時才呼叫（MUI2 Pages\Finish.nsh 的 Leave 函式），**晚於整個 Section Install**，
;     POSTINSTALL 時捷徑還不存在。
; 範本之後沒有任何 NSIS_HOOK_* 巨集，所以互動安裝改在 .onGUIEnd 處理：NSIS 在安裝視窗的 DialogBox
; 返回（所有頁面含完成頁的 Leave 都跑完）之後才呼叫它（NSIS 3.11 Source/exehead/Ui.c ui_doinstall），
; 靜默安裝沒有視窗、不呼叫它（同函式的 silent 分支），由 POSTINSTALL 那次處理。範本與 MUI2 都沒有定義
; .onGUIEnd；日後範本若自己定義，makensis 會以「函式重複定義」編譯失敗，不會靜默失效。
;
; 函式本體在 !include 當下編譯，範本的 define 還沒宣告（見上方 un. 函式的說明），所以路徑由 POSTINSTALL
; 先填進 $FcDeskSrc／$FcDeskDst／$FcDeskExe。IsShortcutTarget（utils.nsh:164-188，改動 $0-$3）在
; installer.nsi:26 就已 include，早於本檔（:36）。
;
; 規則（只動「指向本次安裝主程式」的捷徑，與範本解除安裝的判準相同，installer.nsi:844-849）：
;   - 來源 fc-host.lnk 不存在，或目標不是本主程式：什麼都不做（更新模式、/NS、使用者沒勾都屬此類）；
;   - 已有「財經日曆.lnk」且目標是本主程式（同版本重裝、覆蓋安裝）：先刪再改名——Rename 遇到已存在的
;     目標會失敗，不先刪就會留下兩個捷徑；
;   - 已有「財經日曆.lnk」但目標不是本主程式（使用者自己的捷徑）：不刪、不改名，保留 fc-host.lnk。
; ------------------------------------------------------------------------------------------------
Function FcRenameDesktopShortcut
  ${IfNot} ${FileExists} "$FcDeskSrc"
    Return
  ${EndIf}
  !insertmacro IsShortcutTarget "$FcDeskSrc" "$FcDeskExe"
  Pop $0
  ${If} $0 <> 1
    Return
  ${EndIf}
  ${If} ${FileExists} "$FcDeskDst"
    !insertmacro IsShortcutTarget "$FcDeskDst" "$FcDeskExe"
    Pop $0
    ${If} $0 <> 1
      Return
    ${EndIf}
    Delete "$FcDeskDst"
  ${EndIf}
  Rename "$FcDeskSrc" "$FcDeskDst"
  ClearErrors
FunctionEnd

; 互動安裝：完成頁可能剛建立 fc-host.lnk，在這裡改名。$FcInstallDone 只在 POSTINSTALL 跑過才是 1，
; 使用者中途取消時（.onGUIEnd 同樣會被呼叫）不動桌面。被動模式也有視窗、也會走到這裡，此時捷徑已在
; POSTINSTALL 改過名，來源不存在、什麼都不做。
Function .onGUIEnd
  ${If} $FcInstallDone = 1
    Call FcRenameDesktopShortcut
  ${EndIf}
FunctionEnd

; ------------------------------------------------------------------------------------------------
; NSIS_HOOK_PREINSTALL：記下安裝前是否已有主程式。
;
; 為什麼要記：POSTINSTALL 只在「首次安裝」才寫開機自啟 Run 值，才不會蓋掉使用者在設定視窗關掉自啟的
; 選擇。同版本「直接覆蓋」不會先解除安裝（PageLeaveReinstall 同版本的預設選項是 add/reinstall，
; installer.nsi:328-330），此時 $UpdateMode 也不是 1，單看 $UpdateMode 分不出「首次」與「覆蓋」。
; 這裡是複製檔案之前（installer.nsi:643 在 :649 `File` 之前），主程式若已存在＝既有安裝。
; ------------------------------------------------------------------------------------------------
!macro NSIS_HOOK_PREINSTALL
  ${If} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
    StrCpy $FcHadInstall 1
  ${Else}
    StrCpy $FcHadInstall 0
  ${EndIf}
!macroend

; ------------------------------------------------------------------------------------------------
; NSIS_HOOK_POSTINSTALL
; ------------------------------------------------------------------------------------------------
!macro NSIS_HOOK_POSTINSTALL
  ; (1) 每次都覆寫「應用程式與功能」的顯示名稱（含更新模式）。
  ; 範本每次安裝都在 installer.nsi:702 把 DisplayName 寫回 ${PRODUCTNAME}（fc-host）；POSTINSTALL 在
  ; :735，晚於它，所以在這裡覆寫才不會被改回去。SHCTX 在 currentUser 模式＝HKCU：.onInit 呼叫
  ; SetContext（installer.nsi:498），其中 `SetShellVarContext current`（utils.nsh:4-5）；
  ; ${UNINSTKEY}＝Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}（installer.nsi:67）。
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayName" "財經日曆"

  ; (2) 開始功能表捷徑改名：fc-host.lnk → 財經日曆.lnk。
  ; 範本在 CreateOrUpdateStartMenuShortcut（installer.nsi:915-955）建立 ${PRODUCTNAME}.lnk；
  ; 更新模式（或 /NS）下它在 :941-943 直接 Return、不建立。所以只在「來源捷徑存在」時才動作：
  ;   - 來源存在（首次安裝、同版本重裝）：先刪掉既有的「財經日曆.lnk」，再把來源 Rename 過去——
  ;     Rename 遇到已存在的目標會失敗，不先刪的話重裝時改名不成、留下兩個捷徑；
  ;   - 來源不存在（更新模式）：什麼都不做。若這時仍先刪目標，更新後捷徑就消失了。
  ; 範本的捷徑建立位置在 STARTMENUFOLDER 為空時是 $SMPROGRAMS\${PRODUCTNAME}.lnk（:951-953）；
  ; 若日後設了 startMenuFolder，捷徑會改放子資料夾（:947-950），這裡的路徑就不對了，所以編譯期擋下。
  !if "${STARTMENUFOLDER}" != ""
    !error "hooks.nsh: tauri.conf.json 設了 startMenuFolder，捷徑改名路徑要跟著改"
  !endif
  ${If} ${FileExists} "${FC_SM_DIR}\${PRODUCTNAME}.lnk"
    Delete "${FC_SM_DIR}\財經日曆.lnk"
    Rename "${FC_SM_DIR}\${PRODUCTNAME}.lnk" "${FC_SM_DIR}\財經日曆.lnk"
    ClearErrors
  ${EndIf}

  ; (3) 開機自啟 Run 值：只在「非更新模式、且安裝前沒有既有主程式」才寫。
  ; 值名＝${PRODUCTNAME}（fc-host），與範本解除安裝刪除的值（installer.nsi:866）、宿主在設定視窗切換
  ; 自啟時寫的值（host/src/widgets.rs 的 `"<exe>" --autostart`）一致。更新模式與覆蓋安裝都不碰它，
  ; 保留使用者在設定視窗關掉自啟的選擇。HKCU 是明寫（同範本 :866），不依賴 SHCTX。
  ${If} $UpdateMode <> 1
  ${AndIf} $FcHadInstall = 0
    WriteRegStr HKCU "${FC_RUN_KEY}" "${PRODUCTNAME}" '"$INSTDIR\${MAINBINARYNAME}.exe" --autostart'
  ${EndIf}

  ; (4) 桌面捷徑改名：fc-host.lnk → 財經日曆.lnk（規則見上方 FcRenameDesktopShortcut）。
  ; 被動／靜默模式的捷徑在 installer.nsi:729-731 已建好，這裡就改名；互動模式的捷徑要等完成頁
  ; （:411-413）才建立，由 .onGUIEnd 再呼叫一次。是否建立桌面捷徑仍完全由範本決定，這裡只改名稱。
  StrCpy $FcDeskSrc "${FC_DESKTOP_DIR}\${PRODUCTNAME}.lnk"
  StrCpy $FcDeskDst "${FC_DESKTOP_DIR}\財經日曆.lnk"
  StrCpy $FcDeskExe "$INSTDIR\${MAINBINARYNAME}.exe"
  Call FcRenameDesktopShortcut
  StrCpy $FcInstallDone 1
!macroend

; ------------------------------------------------------------------------------------------------
; NSIS_HOOK_PREUNINSTALL（只在非更新模式）：還原桌布。
;
; 位置：Section Uninstall 的第一件事（installer.nsi:779-781），早於 CheckIfAppIsRunning（:783）——
; 此時宿主可能仍在執行，--restore-wallpaper 會經 single-instance 通道把還原交給它
; （dynamic-wallpaper 4.7b）；範本之後的 Restart Manager 才會結束它。
; 非更新模式判斷用 $UpdateMode（un.onInit 設，installer.nsi:771-774）；$PassiveMode 同理（:766-769）。
; 記錄目錄的 $LOCALAPPDATA 是 NSIS 內建的每使用者路徑（範本自己在 :515、:884 也這樣用），
; ${BUNDLEID}＝tw.fintools.fc-host（installer.nsi:55，與宿主的資料／記錄目錄同名）。
; 解除安裝若勾了「刪除應用程式資料」，範本會在本 hook 之後（:871-885）整個刪掉該目錄，記錄也隨之刪除，
; 這是使用者明確要求的結果。
; ------------------------------------------------------------------------------------------------
!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    StrCpy $FcExe "$INSTDIR\${MAINBINARYNAME}.exe"
    !ifdef FC_TEST_LOG_DIR
      StrCpy $FcLogDir "${FC_TEST_LOG_DIR}"
    !else
      StrCpy $FcLogDir "$LOCALAPPDATA\${BUNDLEID}\logs"
    !endif
    StrCpy $FcWait3Ms "${FC_WAIT_CODE3_MS}"
    StrCpy $FcWait4Ms "${FC_WAIT_CODE4_MS}"
    StrCpy $FcWait1Ms "${FC_WAIT_CODE1_MS}"
    ; silent（/S）與 passive（/P）都不跳訊息框，只寫記錄
    StrCpy $FcShowUi 1
    ${If} ${Silent}
      StrCpy $FcShowUi 0
    ${EndIf}
    ${If} $PassiveMode = 1
      StrCpy $FcShowUi 0
    ${EndIf}
    Call un.FcRestoreWallpaper
  ${EndIf}
!macroend

; ------------------------------------------------------------------------------------------------
; NSIS_HOOK_POSTUNINSTALL（只在非更新模式）
; ------------------------------------------------------------------------------------------------
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    ; (1) 刪「財經日曆.lnk」。範本只刪 ${PRODUCTNAME}.lnk（installer.nsi:836-841，且要捷徑目標吻合），
    ; 改名後的捷徑它不認得。沿用範本的 UnpinShortcut（utils.nsh:124-134，installer.nsi:839 同樣用法）
    ; 先從開始功能表／工作列取消釘選再刪；不比對捷徑目標——這個名稱是本 hook 自己在 POSTINSTALL 取的。
    ${If} ${FileExists} "${FC_SM_DIR}\財經日曆.lnk"
      !insertmacro UnpinShortcut "${FC_SM_DIR}\財經日曆.lnk"
      Delete "${FC_SM_DIR}\財經日曆.lnk"
    ${EndIf}

    ; (1b) 刪桌面的「財經日曆.lnk」（POSTINSTALL／.onGUIEnd 改名來的）。範本只刪桌面的 ${PRODUCTNAME}.lnk
    ; （installer.nsi:844-849，要求目標吻合；舊版留下的 fc-host.lnk 由它清掉）。桌面是使用者自己的空間，
    ; 同名捷徑可能是使用者自建、指向別處，所以比照範本**要求目標是本主程式**才取消釘選並刪除。
    ; IsShortcutTarget 會改動 $0-$3；本巨集之後範本只剩 SetAutoClose（installer.nsi:891-895）。
    ${If} ${FileExists} "${FC_DESKTOP_DIR}\財經日曆.lnk"
      !insertmacro IsShortcutTarget "${FC_DESKTOP_DIR}\財經日曆.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
      Pop $0
      ${If} $0 = 1
        !insertmacro UnpinShortcut "${FC_DESKTOP_DIR}\財經日曆.lnk"
        Delete "${FC_DESKTOP_DIR}\財經日曆.lnk"
      ${EndIf}
    ${EndIf}

    ; (2) 刪 StartupApproved\Run 的 fc-host 值：使用者曾在工作管理員停用自啟時，系統把停用狀態存在這裡，
    ; 範本只刪 Run 值（installer.nsi:866），不刪這個；不清的話，重新安裝後 Run 值寫回了、自啟卻仍被停用，
    ; 宿主設定視窗的開關還顯示「開」。（解除安裝刪 Run 值由範本內建，這裡不重複做。）
    DeleteRegValue HKCU "${FC_STARTUP_APPROVED_KEY}" "${PRODUCTNAME}"
  ${EndIf}
!macroend
