<#
.SYNOPSIS
    host/tools 驗收／探針腳本共用的「鎖定安全」輸入注入模組（task 1.1 fix round 1）。

.DESCRIPTION
    **本模組是 host/tools/ 下唯一允許呼叫底層輸入注入 API 的地方**；其他腳本一律改用這裡的
    包裝函式，不得自行 DllImport 鍵盤／滑鼠注入函式。

    安全規則（global-constraints 第 18 條；memory input-injection-while-locked-locks-account）：
      - 工作階段鎖定時注入的按鍵會打進登入畫面的密碼框，累積錯誤登入並凍結帳戶。
      - 鎖定判準**只看 LogonUI.exe**（LockApp 行程解鎖後可能殘留，不能用它判斷）。
      - **每一次**底層注入呼叫（每個按鍵按下／放開、每個滑鼠按下／放開／滾輪格、每次游標移動）
        之前都重新檢查一次，不只在一組動作的開頭檢查——動作之間的等待期間可能剛好鎖定
        （Codex task 1.1 r1 [high]）。
      - 偵測到鎖定（或檢查本身失敗＝fail-closed）就丟出終止例外，訊息以 `BLOCKED:` 開頭；
        模組同時進入「已停止」狀態，之後即使解鎖，所有注入呼叫也一律丟 BLOCKED，不重試。
        要恢復只能開新的 PowerShell 行程（或測試用的 Reset-SafeInputBackend）。
      - 唯一例外（controller 裁決，fix round 2）：進入停止狀態的當下，先把「本行程已按下、
        尚未放開」的鍵／滑鼠鍵依相反順序送出放開（不重查鎖定、只送 key-up／mouse-up），
        再丟 BLOCKED。不允許任何新的按下、滾輪或游標移動。

      - **「檢查→注入」之間不得有長耗時步驟**（fix round 3，Codex 1.1fix r1 [high]）：原生後端
        （user32 宣告，Add-Type 編譯要數秒）在**匯入模組時**就編譯好；每次注入的順序固定為
        「Prepare（已初始化即 no-op）→ 鎖定檢查 → 立即呼叫底層 API」，注入用的 scriptblock 內
        不再做任何初始化。
      - **合成輸入是否真的生效**（fix round 3；memory logonui-unlocked-but-synthetic-input-inert）：
        LogonUI 不在只代表「不在密碼畫面」，提權視窗卡在前景時合成輸入會對整台機器靜默失效。
        注入腳本開始時一律呼叫 Invoke-SafeInputPreflight：送一次無害的 F15（VK 0x7E）按下／放開，
        查 GetLastInputInfo 的 dwTime 有沒有前進；沒前進就回傳結束碼 3（ENV-BLOCKED），腳本
        照此結束，不產生「無資訊 PASS」。結束碼約定：0＝可繼續、2＝BLOCKED（鎖定）、3＝ENV-BLOCKED。
      - **前景是系統 UI 時不送任何鍵**（fix F8，批次 B 補跑：「Windows 安全性」對話框佔住前景，F15 會送進它）：
        前置探查在送 F15 之前先查前景根視窗（類別、行程、cloaked、topmost），以 lib/SystemUi.psm1 的
        Get-ForegroundBlockReason 判斷（與 Occluders 的系統 UI 規則同源）。系統 UI 或無法判斷（fail closed）
        → 結束碼 3（ENV-BLOCKED），訊息記前景類別與行程名，零注入。

    可替換後端（只供 host/tools/tests/ 的 mock 測試使用）：初始化、鎖定檢查、最後輸入時間、
    前景描述、前景資訊、按鍵、滑鼠、游標、等待等 scriptblock 可用 Set-SafeInputBackend 換掉，測試因此
    不必呼叫任何真實注入 API（匯入時仍會編譯原生型別，但編譯本身不注入）。

.EXAMPLE
    Import-Module (Join-Path $PSScriptRoot 'lib\SafeInput.psm1') -Force
    $pf = Invoke-SafeInputPreflight
    if ($pf.ExitCode -ne 0) { Write-Host $pf.Message; exit $pf.ExitCode }
    Send-GuardedWinD
    Send-GuardedClick 1200 800
#>
Set-StrictMode -Version Latest
# fix F8：前景是否為系統 UI 的判斷與 lib/Occluders.psm1 共用（Get-ForegroundBlockReason）。
Import-Module (Join-Path $PSScriptRoot 'SystemUi.psm1')

# ---------------------------------------------------------------- 真實後端（user32）

function Initialize-SafeInputNative {
    if ('SafeInputNative.User32' -as [type]) { return }
    Add-Type -Namespace SafeInputNative -Name User32 -MemberDefinition @'
[DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, System.UIntPtr extra);
[DllImport("user32.dll")] public static extern void mouse_event(uint flags, int dx, int dy, uint data, System.UIntPtr extra);
[DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
[DllImport("user32.dll")] public static extern bool GetLastInputInfo(ref LASTINPUTINFO plii);
[DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hWnd, out uint pid);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(System.IntPtr hWnd, System.Text.StringBuilder s, int n);
// fix F8：前置探查送鍵之前查前景根視窗的類別、行程、cloaked、topmost（唯讀查詢）。
[DllImport("user32.dll")] public static extern System.IntPtr GetAncestor(System.IntPtr hWnd, uint flags);
[DllImport("user32.dll", EntryPoint = "GetWindowLongW")] public static extern int GetWindowLong(System.IntPtr hWnd, int index);
[DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(System.IntPtr hWnd, int attr, out int value, int size);
[StructLayout(LayoutKind.Sequential)] public struct LASTINPUTINFO { public uint cbSize; public uint dwTime; }
// fix F8b：前景是桌面時，確認它屬殼層 explorer，並列出可見且覆蓋整個螢幕的頂層視窗（唯讀查詢）。
[DllImport("user32.dll")] public static extern System.IntPtr GetShellWindow();
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr hWnd, out RECT r);
[DllImport("user32.dll")] public static extern System.IntPtr MonitorFromWindow(System.IntPtr hWnd, uint flags);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern bool GetMonitorInfo(System.IntPtr hMonitor, ref MONITORINFO mi);
public delegate bool EnumWindowsProc(System.IntPtr hWnd, System.IntPtr lParam);
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, System.IntPtr lParam);
[StructLayout(LayoutKind.Sequential)] public struct MONITORINFO { public int cbSize; public RECT rcMonitor; public RECT rcWork; public uint dwFlags; }
// 可見的頂層視窗（Z 序由上而下）。
public static System.IntPtr[] VisibleTopLevelWindows() {
    var list = new System.Collections.Generic.List<System.IntPtr>();
    EnumWindows((h, l) => { if (IsWindowVisible(h)) list.Add(h); return true; }, System.IntPtr.Zero);
    return list.ToArray();
}
// 1＝視窗矩形涵蓋其所在螢幕（MONITOR_DEFAULTTONEAREST）的整個 rcMonitor、0＝沒有、-1＝讀不到。
public static int CoversMonitor(System.IntPtr hWnd) {
    RECT r;
    if (!GetWindowRect(hWnd, out r)) return -1;
    System.IntPtr mon = MonitorFromWindow(hWnd, 2);
    if (mon == System.IntPtr.Zero) return -1;
    MONITORINFO mi = new MONITORINFO();
    mi.cbSize = Marshal.SizeOf(typeof(MONITORINFO));
    if (!GetMonitorInfo(mon, ref mi)) return -1;
    return (r.Left <= mi.rcMonitor.Left && r.Top <= mi.rcMonitor.Top && r.Right >= mi.rcMonitor.Right && r.Bottom >= mi.rcMonitor.Bottom) ? 1 : 0;
}

// ------------------------------------------------------------ 合成觸控（task B2/B7 代驗，2026-09-29）
// CreateSyntheticPointerDevice／InjectSyntheticPointerInput／DestroySyntheticPointerDevice：
// Microsoft Learn 一手文件（winuser.h，Windows 10 1809+），非觸控板／滾輪注入 API，故獨立於
// 上面既有的 keybd_event／mouse_event 之外，但同樣只准在本模組內宣告與呼叫。
// POINTER_INFO／POINTER_TOUCH_INFO／POINTER_TYPE_INFO 逐欄位對照 Microsoft Learn 文件（
// ns-winuser-pointer_info／ns-winuser-pointer_touch_info／ns-winuser-pointer_type_info）
// 原樣還原：POINTER_TYPE_INFO 的第二個成員是 C union（pointerInfo／touchInfo／penInfo 共用同一塊
// 記憶體），這裡只用得到 touchInfo，故直接用 Sequential 佈局把 touchInfo 接在 type 後面——
// POINTER_INFO 內含 HANDLE／HWND（8 位元組、需 8 位元組對齊），CLR 會在 type（4 位元組）後自動
// 補 4 位元組 padding 才放 touchInfo，這與原生 union 因同一對齊需求而產生的 offset 相同，故此處
// 的循序佈局與真正的 union 版面相容（僅限只使用 touchInfo 這個分支的情況，不通用於 penInfo）。
[DllImport("user32.dll", SetLastError = true)] public static extern System.IntPtr CreateSyntheticPointerDevice(uint pointerType, uint maxCount, uint mode);
[DllImport("user32.dll", SetLastError = true)] public static extern bool InjectSyntheticPointerInput(System.IntPtr device, ref POINTER_TYPE_INFO pointerInfo, uint count);
[DllImport("user32.dll")] public static extern bool DestroySyntheticPointerDevice(System.IntPtr device);
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
[StructLayout(LayoutKind.Sequential)]
public struct POINTER_INFO {
    public int pointerType;
    public uint pointerId;
    public uint frameId;
    public int pointerFlags;
    public System.IntPtr sourceDevice;
    public System.IntPtr hwndTarget;
    public POINT ptPixelLocation;
    public POINT ptHimetricLocation;
    public POINT ptPixelLocationRaw;
    public POINT ptHimetricLocationRaw;
    public uint dwTime;
    public uint historyCount;
    public int InputData;
    public uint dwKeyStates;
    public ulong PerformanceCount;
    public int ButtonChangeType;
}
[StructLayout(LayoutKind.Sequential)]
public struct POINTER_TOUCH_INFO {
    public POINTER_INFO pointerInfo;
    public uint touchFlags;
    public uint touchMask;
    public RECT rcContact;
    public RECT rcContactRaw;
    public uint orientation;
    public uint pressure;
}
[StructLayout(LayoutKind.Sequential)]
public struct POINTER_TYPE_INFO {
    public int type;
    public POINTER_TOUCH_INFO touchInfo;
}
// fix F4（review 4.7-minor [high]）：frame 一律在 C# 裡組。PowerShell 對巢狀 struct 的多層賦值
// （$info.touchInfo.pointerInfo.X = v）只改到暫存複本、不報錯，原本送出的 frame 除了 type 以外
// 全是 0（memory powershell-nested-struct-assign-hits-copy）。C# 對區域變數的巢狀欄位賦值是原地修改。
// historyCount：文件「無合併時為 1」。touchMask=0（TOUCH_MASK_NONE）：不使用 rcContact／orientation／pressure。
public static POINTER_TYPE_INFO MakeTouchFrame(uint pointerId, uint frameId, int pointerFlags, int x, int y) {
    POINTER_TYPE_INFO info = new POINTER_TYPE_INFO();
    info.type = 2;  // PT_TOUCH
    info.touchInfo.pointerInfo.pointerType = 2;  // PT_TOUCH
    info.touchInfo.pointerInfo.pointerId = pointerId;
    info.touchInfo.pointerInfo.frameId = frameId;
    info.touchInfo.pointerInfo.pointerFlags = pointerFlags;
    info.touchInfo.pointerInfo.historyCount = 1;
    info.touchInfo.pointerInfo.ptPixelLocation.X = x;
    info.touchInfo.pointerInfo.ptPixelLocation.Y = y;
    info.touchInfo.pointerInfo.ptPixelLocationRaw.X = x;
    info.touchInfo.pointerInfo.ptPixelLocationRaw.Y = y;
    info.touchInfo.touchMask = 0;
    return info;
}
// 送出一個已組好的 frame（只有一次 P/Invoke，鎖定檢查之後立即呼叫）。
public static bool InjectTouchFrame(System.IntPtr device, POINTER_TYPE_INFO frame) {
    return InjectSyntheticPointerInput(device, ref frame, 1);
}
'@
}

function Get-NativeLastInputTick {
    <# GetLastInputInfo 的 dwTime（GetTickCount 基準的毫秒，uint32 會繞回）；呼叫失敗就丟例外。 #>
    $info = New-Object SafeInputNative.User32+LASTINPUTINFO
    $info.cbSize = [uint32][Runtime.InteropServices.Marshal]::SizeOf($info)
    if (-not [SafeInputNative.User32]::GetLastInputInfo([ref]$info)) { throw 'GetLastInputInfo 失敗' }
    return [uint32]$info.dwTime
}

function Get-NativeForegroundDescription {
    <# 前景視窗的類別／PID／行程名／Path（Path 查不到常是「前景為提權行程」的徵兆）。只供診斷訊息用。 #>
    $h = [SafeInputNative.User32]::GetForegroundWindow()
    if ($h -eq [IntPtr]::Zero) { return 'fg=(none)' }
    $sb = New-Object System.Text.StringBuilder 256
    [void][SafeInputNative.User32]::GetClassName($h, $sb, 256)
    $fgPid = [uint32]0
    [void][SafeInputNative.User32]::GetWindowThreadProcessId($h, [ref]$fgPid)
    $p = Get-Process -Id $fgPid -ErrorAction SilentlyContinue
    $path = if ($p -and $p.Path) { $p.Path } else { '(查不到，可能是提權行程)' }
    return ('fg=0x{0:X} class={1} pid={2} name={3} path={4}' -f $h.ToInt64(), $sb.ToString(), $fgPid, $(if ($p) { $p.ProcessName } else { '?' }), $path)
}

function Get-NativeForegroundInfo {
    <#
    fix F8：前景視窗的根視窗（GA_ROOT）資訊，供 Get-ForegroundBlockReason 判斷。查不到的欄位留 $null／空字串，
    由判斷函式一律視為「無法判斷」（fail closed）；這裡不做任何判斷，也不送任何輸入。
    #>
    $fg = [SafeInputNative.User32]::GetForegroundWindow()
    if ($fg -eq [IntPtr]::Zero) { return [PSCustomObject]@{ Hwnd = [IntPtr]::Zero; Class = ''; Pid = 0; ProcessName = ''; Cloaked = $null; ExStyle = $null } }
    $root = [SafeInputNative.User32]::GetAncestor($fg, 2)  # GA_ROOT
    if ($root -eq [IntPtr]::Zero) { $root = $fg }
    $sb = New-Object System.Text.StringBuilder 256
    [void][SafeInputNative.User32]::GetClassName($root, $sb, 256)
    $fgPid = [uint32]0
    [void][SafeInputNative.User32]::GetWindowThreadProcessId($root, [ref]$fgPid)
    $name = try { (Get-Process -Id $fgPid -ErrorAction Stop).ProcessName } catch { '' }
    $cloakedVal = 0
    $hr = [SafeInputNative.User32]::DwmGetWindowAttribute($root, 14, [ref]$cloakedVal, 4)  # DWMWA_CLOAKED
    $cloaked = if ($hr -eq 0) { $cloakedVal -ne 0 } else { $null }
    # GetWindowLongW 回傳有號 int，以位元遮罩轉成無號值。GWL_EXSTYLE=-20。
    $ex = [long][SafeInputNative.User32]::GetWindowLong($root, -20) -band 0xFFFFFFFFL
    $cls = $sb.ToString()
    # fix F8b：殼層 explorer 的 pid（GetShellWindow）；前景是桌面時另列覆蓋整個螢幕的可見頂層視窗。
    $shellPid = [uint32]0
    $shellWnd = [SafeInputNative.User32]::GetShellWindow()
    if ($shellWnd -ne [IntPtr]::Zero) { [void][SafeInputNative.User32]::GetWindowThreadProcessId($shellWnd, [ref]$shellPid) }
    $overlays = $null
    if ($cls -in 'Progman', 'WorkerW') { $overlays = @(Get-NativeScreenOverlays) }
    return [PSCustomObject]@{ Hwnd = $root; Class = $cls; Pid = [int]$fgPid; ProcessName = $name; Cloaked = $cloaked; ExStyle = $ex
        ShellPid = [int]$shellPid; Overlays = $overlays }
}

function Get-NativeScreenOverlays {
    <#
    fix F8b：可見且矩形涵蓋整個所在螢幕的頂層視窗（覆蓋層候選；最大化的一般視窗也會列入，是不是系統 UI
    交給 SystemUi 的 Get-SystemOverlayReason 判斷）。CoversMonitor 讀不到記 $null（判斷時當作覆蓋）。
    #>
    foreach ($h in [SafeInputNative.User32]::VisibleTopLevelWindows()) {
        $cov = [SafeInputNative.User32]::CoversMonitor($h)
        if ($cov -eq 0) { continue }
        $sb = New-Object System.Text.StringBuilder 256
        [void][SafeInputNative.User32]::GetClassName($h, $sb, 256)
        $procId = [uint32]0
        [void][SafeInputNative.User32]::GetWindowThreadProcessId($h, [ref]$procId)
        $pname = try { (Get-Process -Id $procId -ErrorAction Stop).ProcessName } catch { '' }
        $cv = 0
        $hr = [SafeInputNative.User32]::DwmGetWindowAttribute($h, 14, [ref]$cv, 4)  # DWMWA_CLOAKED
        [PSCustomObject]@{
            Hwnd = $h; Class = $sb.ToString(); ProcessName = $pname; Visible = $true
            Cloaked = if ($hr -eq 0) { $cv -ne 0 } else { $null }
            CoversMonitor = if ($cov -eq 1) { $true } else { $null }
        }
    }
}

function New-DefaultSafeInputBackend {
    @{
        # 原生後端在匯入模組時就已編譯（見檔尾）；這裡只是保險，已載入即 no-op。每次注入時
        # 在鎖定檢查「之前」呼叫，確保檢查與注入之間沒有任何編譯。
        Prepare            = { Initialize-SafeInputNative }
        # 鎖定判準只用 LogonUI.exe；Get-Process 找不到時回傳空＝未鎖定。
        IsLocked           = { [bool](Get-Process -Name LogonUI -ErrorAction SilentlyContinue) }
        LastInputTick      = { Get-NativeLastInputTick }
        DescribeForeground = { Get-NativeForegroundDescription }
        ForegroundInfo     = { Get-NativeForegroundInfo }
        # 以下三項是檢查之後立即執行的注入：不得在裡面做初始化（SafeInput.Tests.ps1 靜態檢查）。
        # KEYEVENTF_KEYUP=2
        Key                = { param([byte]$Vk, [uint32]$Flags) [SafeInputNative.User32]::keybd_event($Vk, 0, $Flags, [UIntPtr]::Zero) }
        # MOUSEEVENTF_LEFTDOWN=2、LEFTUP=4、WHEEL=0x0800
        Mouse              = { param([uint32]$Flags, [uint32]$Data) [SafeInputNative.User32]::mouse_event($Flags, 0, 0, $Data, [UIntPtr]::Zero) }
        Cursor             = { param([int]$X, [int]$Y) [void][SafeInputNative.User32]::SetCursorPos($X, $Y) }
        Sleep              = { param([int]$Ms) Start-Sleep -Milliseconds $Ms }
        # PT_TOUCH=2（Microsoft Learn POINTER_INPUT_TYPE）、maxCount=1（單指）、
        # POINTER_FEEDBACK_NONE=3（不顯示系統觸控回饋圈，驗收用途不需要視覺提示）。
        TouchCreateDevice  = { [SafeInputNative.User32]::CreateSyntheticPointerDevice(2, 1, 3) }
        TouchDestroyDevice = { param([IntPtr]$Device) if ($Device -ne [IntPtr]::Zero) { [void][SafeInputNative.User32]::DestroySyntheticPointerDevice($Device) } }
        # 組一個觸控 frame（純記憶體運算、不呼叫 user32）。一律在 C# helper 裡組：PowerShell 的
        # 巢狀 struct 多層賦值只改到複本（fix F4，review 4.7-minor [high]）。frameId 遞增（POINTER_INFO
        # 文件：同一輸入框架共用同一值；遞增比每次都是 0 更貼近真實裝置）。在鎖定檢查「之前」呼叫。
        TouchFrame         = {
            param([uint32]$Flags, [int]$X, [int]$Y, [uint32]$FrameId)
            [SafeInputNative.User32]::MakeTouchFrame([uint32]1, $FrameId, [int]$Flags, $X, $Y)
        }
        # 送出一個已組好的 frame；回傳 InjectSyntheticPointerInput 的 BOOL 結果。檢查之後立即執行，
        # 裡面不得再做組裝或初始化。
        Touch              = { param([IntPtr]$Device, $Frame) [SafeInputNative.User32]::InjectTouchFrame($Device, $Frame) }
    }
}

# 匯入模組時就編譯原生後端（Codex 1.1fix r1 [high]：原本第一次注入才 Add-Type，編譯的數秒落在
# 鎖定檢查之後）。編譯失敗＝匯入失敗，腳本在任何注入之前就停下。
Initialize-SafeInputNative
$script:Backend = New-DefaultSafeInputBackend
# 非 $null＝已偵測到鎖定並停止所有注入；值為觸發當下正要做的動作描述。
$script:Tripped = $null
# 本行程已按下、尚未放開的鍵／滑鼠鍵（依按下順序）：@{ Kind = 'key'|'mouse'; Code = VK 或對應的「放開」flags }。
$script:Pressed = New-Object System.Collections.Generic.List[object]
# 滑鼠「按下」flags → 對應的「放開」flags：LEFT 2→4、RIGHT 8→0x10、MIDDLE 0x20→0x40。
$script:MouseUpFor = @{ [uint32]2 = [uint32]4; [uint32]8 = [uint32]0x10; [uint32]0x20 = [uint32]0x40 }
# 合成觸控每個 frame 遞增的 frameId（見 New-SafeInputTouchFrame）。
$script:TouchFrameId = 0
# 已銷毀的合成觸控裝置 handle（fix F4，review 4.7-minor [medium]/[low]）：ReleaseAll 銷毀過的裝置，
# 拖曳的 finally 不得再銷毀一次；重新建立時若拿到同值 handle 會從集合移除。
$script:DestroyedTouchDevices = New-Object System.Collections.Generic.HashSet[long]

function New-SafeInputTouchFrame {
    <#
    組一個單指觸控 frame（POINTER_TYPE_INFO，PT_TOUCH、pointerId=1、historyCount=1、
    ptPixelLocation 與 ptPixelLocationRaw 皆為 (X,Y)），frameId 逐次遞增。只組、不送出。
    呼叫端（Invoke-GuardedTouch）必須在鎖定檢查「之前」組好，檢查之後立即注入。
    #>
    param([Parameter(Mandatory)][uint32]$Flags, [Parameter(Mandatory)][int]$X, [Parameter(Mandatory)][int]$Y)
    $script:TouchFrameId++
    return (& $script:Backend.TouchFrame $Flags $X $Y ([uint32]$script:TouchFrameId))
}

function Invoke-TouchDeviceDestroy([IntPtr]$Device) {
    <# 銷毀合成觸控裝置，每個 handle 只銷毀一次（已銷毀就不再呼叫）。 #>
    if (-not $script:DestroyedTouchDevices.Add($Device.ToInt64())) { return }
    & $script:Backend.TouchDestroyDevice $Device
}

function Remove-TouchPressedRecord([IntPtr]$Device) {
    for ($i = $script:Pressed.Count - 1; $i -ge 0; $i--) {
        if ($script:Pressed[$i].Kind -eq 'touch' -and $script:Pressed[$i].Device -eq $Device) { $script:Pressed.RemoveAt($i) }
    }
}

function Remove-LastPressed([string]$Kind, [uint32]$Code) {
    for ($i = $script:Pressed.Count - 1; $i -ge 0; $i--) {
        if ($script:Pressed[$i].Kind -eq $Kind -and $script:Pressed[$i].Code -eq $Code) { $script:Pressed.RemoveAt($i); return }
    }
}

function Invoke-SafeInputReleaseAll {
    <#
    controller 裁決（task 1.1 fix round 2）：鎖定時唯一允許的注入＝放開本行程自己按下、尚未
    放開的鍵／滑鼠鍵（只送 key-up／mouse-up，依按下的相反順序，不重查鎖定）。放開不產生字元、
    不會送出密碼；反之卡住的 Win 或滑鼠左鍵會在使用者解鎖後打密碼時造成更大的危險。
    回傳送出的放開數。
    #>
    $n = 0
    for ($i = $script:Pressed.Count - 1; $i -ge 0; $i--) {
        $p = $script:Pressed[$i]
        try {
            if ($p.Kind -eq 'key') { & $script:Backend.Key ([byte]$p.Code) ([uint32]2) }
            elseif ($p.Kind -eq 'touch') {
                # 觸控版的「放開」：送一個 POINTER_FLAG_UP frame（不重查鎖定，同鍵盤／滑鼠），
                # 再銷毀裝置——不留一根手指停在「按住中」的狀態。銷毀會記入
                # DestroyedTouchDevices，拖曳的 finally 不會再銷毀一次。
                try {
                    $upFrame = New-SafeInputTouchFrame ([uint32]0x40000) $p.X $p.Y
                    [void](& $script:Backend.Touch $p.Device $upFrame)
                } finally { Invoke-TouchDeviceDestroy $p.Device }
            }
            else { & $script:Backend.Mouse ([uint32]$p.Code) ([uint32]0) }
            $n++
        } catch { }
    }
    $script:Pressed.Clear()
    return $n
}

# ---------------------------------------------------------------- 鎖定檢查

function Test-SessionLocked {
    <# 工作階段是否鎖定（LogonUI.exe 在跑）。檢查本身失敗一律視為鎖定（fail-closed）。 #>
    try { return [bool](& $script:Backend.IsLocked) } catch { return $true }
}

function Assert-SessionUnlocked {
    <# 未鎖定才返回；鎖定、檢查失敗或先前已停止就丟 BLOCKED 終止例外。 #>
    param([string]$What = '注入')
    if ($null -ne $script:Tripped) {
        throw "BLOCKED: 先前已偵測到工作階段鎖定（於「$($script:Tripped)」），拒絕後續所有注入：$What"
    }
    if (Test-SessionLocked) {
        $script:Tripped = $What
        $released = Invoke-SafeInputReleaseAll
        throw "BLOCKED: LogonUI.exe 在跑（工作階段鎖定），停止注入：$What（已放開本行程按住的 $released 個鍵）"
    }
}

function Get-SafeInputTripped {
    <# 回傳觸發停止時的動作描述；未觸發回傳 $null。 #>
    return $script:Tripped
}

# ---------------------------------------------------------------- 單一底層呼叫（每次都先檢查）
# 固定順序：Invoke-SafeInputPrepare（初始化，已完成即 no-op）→ Assert-SessionUnlocked → 立即注入。
# 初始化只能出現在檢查「之前」，檢查與注入之間不得插入任何可能耗時的步驟。

function Invoke-SafeInputPrepare { & $script:Backend.Prepare }

function Invoke-GuardedKey {
    param([Parameter(Mandatory)][byte]$Vk, [switch]$Up, [string]$What = '按鍵')
    $flags = if ($Up) { [uint32]2 } else { [uint32]0 }
    Invoke-SafeInputPrepare
    Assert-SessionUnlocked ('{0}：VK 0x{1:X2} {2}' -f $What, $Vk, $(if ($Up) { '放開' } else { '按下' }))
    & $script:Backend.Key $Vk $flags
    if ($Up) { Remove-LastPressed 'key' $Vk } else { $script:Pressed.Add(@{ Kind = 'key'; Code = [uint32]$Vk }) }
}

function Invoke-GuardedMouse {
    param([Parameter(Mandatory)][uint32]$Flags, [uint32]$Data = 0, [string]$What = '滑鼠')
    Invoke-SafeInputPrepare
    Assert-SessionUnlocked ('{0}：flags 0x{1:X4}' -f $What, $Flags)
    & $script:Backend.Mouse $Flags $Data
    if ($script:MouseUpFor.ContainsKey($Flags)) { $script:Pressed.Add(@{ Kind = 'mouse'; Code = $script:MouseUpFor[$Flags] }) }
    elseif ($script:MouseUpFor.ContainsValue($Flags)) { Remove-LastPressed 'mouse' $Flags }
}

function Set-GuardedCursorPos {
    param([Parameter(Mandatory)][int]$X, [Parameter(Mandatory)][int]$Y, [string]$What = '移動游標')
    Invoke-SafeInputPrepare
    Assert-SessionUnlocked "$What ($X,$Y)"
    & $script:Backend.Cursor $X $Y
}

function Invoke-SafeInputSleep([int]$Ms) { & $script:Backend.Sleep $Ms }

# ---------------------------------------------------------------- 組合動作

function Send-GuardedWinD {
    <# Win+D（顯示桌面）：VK_LWIN=0x5B、'D'=0x44。每個按下／放開前各自重新檢查鎖定。 #>
    Assert-SessionUnlocked 'Win+D'
    Invoke-GuardedKey 0x5B -What 'Win+D'
    Invoke-GuardedKey 0x44 -What 'Win+D'
    Invoke-SafeInputSleep 40
    Invoke-GuardedKey 0x44 -Up -What 'Win+D'
    Invoke-GuardedKey 0x5B -Up -What 'Win+D'
}

function Send-GuardedAltTap {
    <# 單按一次 Alt（VK_MENU=0x12）：SetForegroundWindow 前景鎖的常見繞法。 #>
    Assert-SessionUnlocked 'Alt（前景鎖繞法）'
    Invoke-GuardedKey 0x12 -What 'Alt（前景鎖繞法）'
    Invoke-GuardedKey 0x12 -Up -What 'Alt（前景鎖繞法）'
}

function Send-GuardedClick {
    <# 移游標到 (X,Y) → 等 80 ms → 左鍵按下 → 等 40 ms → 左鍵放開；等待後都重新檢查。 #>
    param([Parameter(Mandatory)][int]$X, [Parameter(Mandatory)][int]$Y)
    $what = "點擊 ($X,$Y)"
    Assert-SessionUnlocked $what
    Set-GuardedCursorPos $X $Y -What $what
    Invoke-SafeInputSleep 80
    Invoke-GuardedMouse 2 -What "$what 按下"
    Invoke-SafeInputSleep 40
    Invoke-GuardedMouse 4 -What "$what 放開"
}

function Send-GuardedWheel {
    <#
    移游標到 (X,Y) 後送 Notches 格滾輪；Delta 為每格的 WHEEL_DELTA 倍數，負值＝向下捲動
    （內容往上移、scrollTop 增加）。每一格之前都重新檢查鎖定。
    #>
    param([Parameter(Mandatory)][int]$X, [Parameter(Mandatory)][int]$Y, [Parameter(Mandatory)][int]$Notches, [int]$Delta = -120)
    $what = "滾輪 ($X,$Y) x$Notches"
    # mouse_event 的 data 參數是 DWORD：負值以 2 補數表示。
    $data = if ($Delta -lt 0) { [uint32]([int64]4294967296 + $Delta) } else { [uint32]$Delta }
    Assert-SessionUnlocked $what
    Set-GuardedCursorPos $X $Y -What $what
    Invoke-SafeInputSleep 80
    for ($i = 1; $i -le $Notches; $i++) {
        Invoke-GuardedMouse 0x0800 -Data $data -What "$what 第 $i 格"
        Invoke-SafeInputSleep 60
    }
}

# ---------------------------------------------------------------- 合成觸控（PT_TOUCH，task B2/B7）

function New-GuardedTouchDevice {
    <#
    建立一根手指的合成觸控裝置（CreateSyntheticPointerDevice，Microsoft Learn 一手文件，
    Windows 10 1809+）。本身不送任何指標事件，但仍先查鎖定（與其餘組合動作一致的開頭檢查）。
    失敗（回傳 NULL）丟一般例外（非 BLOCKED，不觸發「已停止」狀態——這是 API 呼叫失敗，不是
    鎖定），呼叫端可用 GetLastError 進一步查（本函式不附加，維持單一職責）。
    #>
    Invoke-SafeInputPrepare
    Assert-SessionUnlocked '建立合成觸控裝置'
    $h = & $script:Backend.TouchCreateDevice
    if ($h -eq [IntPtr]::Zero) { throw 'CreateSyntheticPointerDevice 失敗（回傳 NULL），非鎖定造成，見 GetLastError' }
    # 系統可能重用已銷毀的 handle 值：新建立的裝置不得被當成「已銷毀」。
    [void]$script:DestroyedTouchDevices.Remove(([IntPtr]$h).ToInt64())
    return $h
}

function Remove-GuardedTouchDevice {
    <#
    銷毀觸控裝置；已銷毀（例如鎖定時 Invoke-SafeInputReleaseAll 已代為銷毀）就不再呼叫，
    呼叫失敗也不拋例外，供 finally 區塊安心呼叫。
    #>
    param([Parameter(Mandatory)][IntPtr]$Device)
    try { Invoke-TouchDeviceDestroy $Device } catch { }
}

function Invoke-GuardedTouch {
    <#
    單一觸控 frame 注入。Flags 為 POINTER_FLAG_* 組合（Microsoft Learn Pointer Flags 一手文件）：
    DOWN 用 0x10017（NEW|INRANGE|INCONTACT|FIRSTBUTTON|DOWN）、UPDATE 用 0x20016
    （INRANGE|INCONTACT|FIRSTBUTTON|UPDATE）、UP 用 0x40000（UP）——FIRSTBUTTON
    （0x10）依文件「A touch pointer has this flag set when it is in contact with the
    digitizer surface」，觸控在接觸中應一併帶上，本模組 2026-09-29 首版遺漏過這個旗標
    （見 verify-4.7-touch.ps1 對應的除錯記錄／task-4.7-report.md）。每次呼叫前重新檢查鎖定（同其餘
    Invoke-Guarded*）。DOWN 記入 $script:Pressed（供鎖定時 Invoke-SafeInputReleaseAll 代為放開），
    UPDATE 更新其最後座標，UP 移除紀錄——比照鍵盤／滑鼠的「按下即記、放開即消」慣例，但用
    Device 而非 VK/flags 當鍵值（一個裝置在任何時刻只有一根手指按著，maxCount=1）。
    #>
    param(
        [Parameter(Mandatory)][IntPtr]$Device,
        [Parameter(Mandatory)][uint32]$Flags,
        [Parameter(Mandatory)][int]$X,
        [Parameter(Mandatory)][int]$Y,
        [string]$What = '觸控'
    )
    Invoke-SafeInputPrepare
    # fix F4（review 4.7-minor [low]）：frame 在鎖定檢查之前組好，檢查之後只剩一次 P/Invoke。
    $frame = New-SafeInputTouchFrame $Flags $X $Y
    $desc = '{0}：flags 0x{1:X} ({2},{3})' -f $What, $Flags, $X, $Y
    Assert-SessionUnlocked $desc
    $ok = & $script:Backend.Touch $Device $frame
    if (-not $ok) { throw "InjectSyntheticPointerInput 回傳 false（非鎖定造成）：$What" }
    $isDown = ($Flags -band 0x10000) -ne 0
    $isUp = ($Flags -band 0x40000) -ne 0
    if ($isDown) {
        $script:Pressed.Add(@{ Kind = 'touch'; Device = $Device; X = $X; Y = $Y })
    }
    elseif ($isUp) {
        for ($i = $script:Pressed.Count - 1; $i -ge 0; $i--) {
            if ($script:Pressed[$i].Kind -eq 'touch' -and $script:Pressed[$i].Device -eq $Device) { $script:Pressed.RemoveAt($i); break }
        }
    }
    else {
        foreach ($p in $script:Pressed) { if ($p.Kind -eq 'touch' -and $p.Device -eq $Device) { $p.X = $X; $p.Y = $Y } }
    }
}

function Send-GuardedTouchDrag {
    <#
    單指觸控拖曳：按住 (StartX,StartY) → 分 Steps 段線性移動到 (EndX,EndY) → 放開。每個 frame
    之間都重新檢查鎖定（Invoke-GuardedTouch 內部）。裝置在 finally 一定銷毀（正常結束或例外
    皆同），不留殘留的合成觸控裝置。
    fix F4（review 4.7-minor [medium]）：途中發生非鎖定例外（注入回傳 false、Ctrl+C 等）時，
    finally 先對仍按著的這根手指送 UP（照 ReleaseAll 慣例不重查鎖定）並移除紀錄，再銷毀裝置；
    鎖定時 ReleaseAll 已送 UP 並銷毀，finally 不會重複操作。
    #>
    param(
        [Parameter(Mandatory)][int]$StartX, [Parameter(Mandatory)][int]$StartY,
        [Parameter(Mandatory)][int]$EndX, [Parameter(Mandatory)][int]$EndY,
        [int]$Steps = 12, [int]$StepDelayMs = 40
    )
    $what = "觸控拖曳 ($StartX,$StartY)->($EndX,$EndY)"
    Assert-SessionUnlocked $what
    $device = New-GuardedTouchDevice
    try {
        Invoke-GuardedTouch $device ([uint32]0x10017) $StartX $StartY -What "$what 按下"
        Invoke-SafeInputSleep 60
        for ($i = 1; $i -le $Steps; $i++) {
            $t = $i / [double]$Steps
            $x = [int]([Math]::Round($StartX + ($EndX - $StartX) * $t))
            $y = [int]([Math]::Round($StartY + ($EndY - $StartY) * $t))
            Invoke-GuardedTouch $device ([uint32]0x20016) $x $y -What "$what 移動 $i/$Steps"
            Invoke-SafeInputSleep $StepDelayMs
        }
        Invoke-SafeInputSleep 60
        Invoke-GuardedTouch $device ([uint32]0x40000) $EndX $EndY -What "$what 放開"
    }
    finally {
        $held = $null
        foreach ($p in $script:Pressed) { if ($p.Kind -eq 'touch' -and $p.Device -eq $device) { $held = $p } }
        if ($held -and -not $script:DestroyedTouchDevices.Contains(([IntPtr]$device).ToInt64())) {
            try {
                $upFrame = New-SafeInputTouchFrame ([uint32]0x40000) $held.X $held.Y
                [void](& $script:Backend.Touch $device $upFrame)
            } catch { }
        }
        Remove-TouchPressedRecord $device
        Remove-GuardedTouchDevice $device
    }
}

# ---------------------------------------------------------------- 合成輸入是否真的生效（前置探查）

function Test-SyntheticInputEffective {
    <#
    memory logonui-unlocked-but-synthetic-input-inert：LogonUI 不在時，合成輸入仍可能被系統整體
    忽略（例如提權視窗卡在前景），且 keybd_event／mouse_event 沒有回傳值可查。地面真相＝送一次
    無害輸入後 GetLastInputInfo 的 dwTime 有沒有前進。
    無害輸入選 F15（VK_F15＝0x7E；.NET [System.Windows.Forms.Keys]::F15 同值）：一般應用程式
    不綁定、不產生字元，不像 Alt 會叫出選單列。按下／放開都經 Invoke-GuardedKey（照樣逐次查鎖定）。
    回傳 @{ Effective; Before; After; Detail }。GetLastInputInfo 失敗一律視為不生效（fail-closed）。
    #>
    try { $before = [uint32](& $script:Backend.LastInputTick) }
    catch { return [PSCustomObject]@{ Effective = $false; Before = $null; After = $null; Detail = "GetLastInputInfo 失敗：$_" } }
    # 讓系統 tick（約 16 ms 解析度）至少走過一格，避免「同一格內」誤判沒前進。
    Invoke-SafeInputSleep 50
    Invoke-GuardedKey 0x7E -What '合成輸入生效探查（F15）'
    Invoke-GuardedKey 0x7E -Up -What '合成輸入生效探查（F15）'
    Invoke-SafeInputSleep 100
    try { $after = [uint32](& $script:Backend.LastInputTick) }
    catch { return [PSCustomObject]@{ Effective = $false; Before = $before; After = $null; Detail = "GetLastInputInfo 失敗：$_" } }
    # dwTime 是 uint32、約 49.7 天繞回：以模 2^32 的差值判斷「往前」。
    $delta = ([int64]$after - [int64]$before + 4294967296) % 4294967296
    $effective = ($delta -gt 0) -and ($delta -lt 2147483648)
    return [PSCustomObject]@{ Effective = $effective; Before = $before; After = $after; Detail = "dwTime $before → $after（差 $delta ms）" }
}

function Invoke-SafeInputPreflight {
    <#
    注入腳本開始時一律呼叫（fix round 3）。回傳 @{ ExitCode; Message }：
      0＝已解鎖且合成輸入確實生效，可繼續；
      2＝BLOCKED（LogonUI 在跑，或探查途中鎖定——模組已進入停止狀態）；
      3＝ENV-BLOCKED（未鎖定，但合成輸入沒有被系統計入＝後續任何 PASS/FAIL 都不具資訊）。
    用法：$pf = Invoke-SafeInputPreflight; if ($pf.ExitCode -ne 0) { Write-Host $pf.Message; exit $pf.ExitCode }
    #>
    try {
        Assert-SessionUnlocked '前置探查'
        # fix F8：送任何鍵之前先查前景；系統 UI 或無法判斷 → ENV-BLOCKED，零注入。
        $fgInfo = $null; $fgErr = ''
        try { $fgInfo = & $script:Backend.ForegroundInfo } catch { $fgErr = "前景查詢失敗：$_" }
        $why = if ($fgErr) { $fgErr } else { Get-ForegroundBlockReason -Window $fgInfo }
        if ($why) {
            $cls = if ($null -ne $fgInfo -and $fgInfo.PSObject.Properties['Class']) { $fgInfo.Class } else { '?' }
            $proc = if ($null -ne $fgInfo -and $fgInfo.PSObject.Properties['ProcessName']) { $fgInfo.ProcessName } else { '?' }
            $hw = if ($null -ne $fgInfo -and $fgInfo.PSObject.Properties['Hwnd'] -and $null -ne $fgInfo.Hwnd) { '0x{0:X}' -f ([IntPtr]$fgInfo.Hwnd).ToInt64() } else { '?' }
            return [PSCustomObject]@{
                ExitCode = 3
                Message  = "ENV-BLOCKED: 前景是系統 UI 或無法判斷（$why；fg=$hw class=$cls process=$proc），未送任何鍵。需要有人關閉該系統視窗或把一般視窗切到前景後再重跑；本次不做任何驗收判讀。"
            }
        }
        $r = Test-SyntheticInputEffective
    } catch {
        if ("$_" -like 'BLOCKED*') { return [PSCustomObject]@{ ExitCode = 2; Message = "$_" } }
        throw
    }
    if (-not $r.Effective) {
        $fg = try { & $script:Backend.DescribeForeground } catch { "前景查詢失敗：$_" }
        return [PSCustomObject]@{
            ExitCode = 3
            Message  = "ENV-BLOCKED: 工作階段未鎖定，但合成輸入沒有被系統計入（$($r.Detail)；$fg）。常見原因：提權視窗卡在前景（UIPI）。需要有人實際動一下鍵盤／滑鼠或排除該視窗後再重跑；本次不做任何驗收判讀。"
        }
    }
    return [PSCustomObject]@{ ExitCode = 0; Message = "OK: 合成輸入生效（$($r.Detail)）" }
}

# ---------------------------------------------------------------- 測試用：替換後端

function Set-SafeInputBackend {
    <#
    只供 mock 測試使用：替換初始化／鎖定檢查／最後輸入時間／前景描述／按鍵／滑鼠／游標／等待
    後端。未指定的項目維持原值，所以測試應全部都給，確保不會落到真實後端。
    #>
    param([scriptblock]$Prepare, [scriptblock]$IsLocked, [scriptblock]$LastInputTick, [scriptblock]$DescribeForeground,
        [scriptblock]$ForegroundInfo,
        [scriptblock]$Key, [scriptblock]$Mouse, [scriptblock]$Cursor, [scriptblock]$Sleep,
        [scriptblock]$TouchCreateDevice, [scriptblock]$TouchDestroyDevice, [scriptblock]$TouchFrame, [scriptblock]$Touch)
    foreach ($name in 'Prepare', 'IsLocked', 'LastInputTick', 'DescribeForeground', 'ForegroundInfo', 'Key', 'Mouse', 'Cursor', 'Sleep',
        'TouchCreateDevice', 'TouchDestroyDevice', 'TouchFrame', 'Touch') {
        $sb = Get-Variable -Name $name -ValueOnly
        if ($sb) { $script:Backend[$name] = $sb }
    }
    $script:Tripped = $null
    $script:Pressed.Clear()
    $script:DestroyedTouchDevices.Clear()
}

function Reset-SafeInputBackend {
    <# 只供 mock 測試使用：恢復真實後端並清除「已停止」狀態、按住集合與已銷毀裝置紀錄。 #>
    $script:Backend = New-DefaultSafeInputBackend
    $script:Tripped = $null
    $script:Pressed.Clear()
    $script:DestroyedTouchDevices.Clear()
}

Export-ModuleMember -Function Test-SessionLocked, Assert-SessionUnlocked, Get-SafeInputTripped,
Invoke-GuardedKey, Invoke-GuardedMouse, Set-GuardedCursorPos, Invoke-SafeInputSleep,
Send-GuardedWinD, Send-GuardedAltTap, Send-GuardedClick, Send-GuardedWheel,
New-GuardedTouchDevice, Remove-GuardedTouchDevice, Invoke-GuardedTouch, Send-GuardedTouchDrag, New-SafeInputTouchFrame,
Test-SyntheticInputEffective, Invoke-SafeInputPreflight,
Set-SafeInputBackend, Reset-SafeInputBackend
