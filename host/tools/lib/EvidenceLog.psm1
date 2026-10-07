<#
.SYNOPSIS
    驗收證據去識別：記錄檔裡的使用者設定檔路徑一律改寫成環境變數字樣（task 7.7）。

.DESCRIPTION
    repo 公開，證據記錄檔不得留下本機使用者設定檔路徑（`C:` 磁碟、`Users` 目錄、使用者名稱
    那一段）。本模組把 `%TEMP%`、`%LOCALAPPDATA%`、`%APPDATA%`、`%USERPROFILE%` 四個目錄
    （由長到短比對，先比對 TEMP）改寫成對應的字樣，分隔字元不論是 `\`、JSON 跳脫後的 `\\`、
    `\\\\` 或 `/` 都認得，不分大小寫。路徑改寫之後，再把不在路徑裡的裸使用者名稱與電腦名稱
    （前後不緊接英數或底線、至少 4 字元）改寫成 `%USERNAME%`、`%COMPUTERNAME%`。四個目錄的 8.3
    短路徑（GetShortPathNameW）也列入路徑規則。

    - `New-EvidenceWriter <路徑>`：取代 `New-Object System.IO.StreamWriter(<路徑>, $false)`，
      回傳的 writer 在 `Write(string)`／`WriteLine(string)` 寫出前先改寫路徑（UTF-8 無 BOM、
      覆寫，與原本的建構子相同）。
    - `ConvertTo-EvidenceText <字串>`：只改寫字串（給 `Set-Content`／`Add-Content` 用）。
    - `Protect-EvidenceFile <路徑...>`：就地改寫既有文字檔（保留 BOM 與行尾；子行程寫的記錄
      或歷史證據用）。回傳實際有改動的檔案數。
    - `Copy-EvidenceFile -Source <來源> -Destination <證據檔>`：複製後立即 `Protect-EvidenceFile`；
      驗收腳本把子行程的記錄檔放進證據目錄一律經它，不直接 `Copy-Item`。
#>
Set-StrictMode -Version Latest

if (-not ('EvidenceLog.PathScrubber' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Text;
using System.Text.RegularExpressions;

namespace EvidenceLog {
    public static class PathScrubber {
        static readonly List<KeyValuePair<Regex, string>> Rules = new List<KeyValuePair<Regex, string>>();

        // 目錄字串 → 正規式：每個分隔字元改成「一個以上反斜線或一個斜線」，其餘字元照字面比對。
        static Regex Build(string dir) {
            var parts = dir.TrimEnd('\\', '/').Split(new[] { '\\', '/' }, StringSplitOptions.RemoveEmptyEntries);
            var sb = new StringBuilder();
            for (int i = 0; i < parts.Length; i++) {
                if (i > 0) sb.Append(@"(?:\\+|/)");
                sb.Append(Regex.Escape(parts[i]));
            }
            // 後面不能緊接著路徑名稱字元（避免 C:\Users\ab 誤中 C:\Users\abc）。
            sb.Append(@"(?![A-Za-z0-9_.~-])");
            return new Regex(sb.ToString(), RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);
        }

        public static void Configure(string[] dirs, string[] tokens) {
            Rules.Clear();
            var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            for (int i = 0; i < dirs.Length; i++) {
                var d = dirs[i];
                if (string.IsNullOrWhiteSpace(d)) continue;
                d = d.TrimEnd('\\', '/');
                if (d.Length < 4 || !seen.Add(d)) continue;
                Rules.Add(new KeyValuePair<Regex, string>(Build(d), tokens[i]));
            }
        }

        static readonly List<KeyValuePair<Regex, string>> NameRules = new List<KeyValuePair<Regex, string>>();

        // 裸名稱（使用者名稱、電腦名稱）：前後都不能緊接名稱字元，路徑規則之後才套用。
        public static void ConfigureNames(string[] names, string[] tokens) {
            NameRules.Clear();
            for (int i = 0; i < names.Length; i++) {
                var n = names[i];
                if (string.IsNullOrWhiteSpace(n) || n.Length < 4) continue;
                // 只排除英數與底線：句尾的句點、主機名稱延伸的 `-PC`／`.lan` 仍要改寫（審查 low）。
                var re = new Regex(@"(?<![A-Za-z0-9_])" + Regex.Escape(n) + @"(?![A-Za-z0-9_])",
                    RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);
                NameRules.Add(new KeyValuePair<Regex, string>(re, tokens[i]));
            }
        }

        public static string Apply(string s) {
            if (string.IsNullOrEmpty(s)) return s;
            foreach (var r in Rules) s = r.Key.Replace(s, r.Value);
            foreach (var r in NameRules) s = r.Key.Replace(s, r.Value);
            return s;
        }
    }

    public static class ShortPath {
        [System.Runtime.InteropServices.DllImport("kernel32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode, SetLastError = true)]
        static extern uint GetShortPathNameW(string lpszLongPath, StringBuilder lpszShortPath, uint cchBuffer);
        // 8.3 短路徑；取不到（不存在、磁碟停用短檔名）回傳 null。
        public static string Get(string path) {
            if (string.IsNullOrEmpty(path)) return null;
            var sb = new StringBuilder(1024);
            uint n = GetShortPathNameW(path, sb, (uint)sb.Capacity);
            if (n == 0 || n >= sb.Capacity) return null;
            return sb.ToString();
        }
    }

    public class ScrubbingWriter : System.IO.StreamWriter {
        public ScrubbingWriter(string path) : base(path, false) { }
        public override void Write(string value) { base.Write(PathScrubber.Apply(value)); }
        public override void WriteLine(string value) { base.WriteLine(PathScrubber.Apply(value)); }
    }
}
'@
}

function Get-LongPath([string]$Path) {
    if ([string]::IsNullOrWhiteSpace($Path)) { return $null }
    try { return (Get-Item -LiteralPath $Path -ErrorAction Stop).FullName } catch { return $Path }
}

function Get-ShortPath([string]$Path) {
    if ([string]::IsNullOrWhiteSpace($Path)) { return $null }
    return [EvidenceLog.ShortPath]::Get($Path)
}

# 比對順序＝由最具體到最一般（TEMP 在 LOCALAPPDATA 底下、兩個 AppData 在 USERPROFILE 底下）。
# 同一個目錄的長檔名與 8.3 短檔名（GetShortPathNameW；審查 low）都列入。
$pairs = @(
    @([IO.Path]::GetTempPath(), '%TEMP%'),
    @($env:TEMP, '%TEMP%'),
    @((Get-LongPath $env:TEMP), '%TEMP%'),
    @((Get-ShortPath $env:TEMP), '%TEMP%'),
    @($env:LOCALAPPDATA, '%LOCALAPPDATA%'),
    @((Get-ShortPath $env:LOCALAPPDATA), '%LOCALAPPDATA%'),
    @($env:APPDATA, '%APPDATA%'),
    @((Get-ShortPath $env:APPDATA), '%APPDATA%'),
    @($env:USERPROFILE, '%USERPROFILE%'),
    @((Get-LongPath $env:USERPROFILE), '%USERPROFILE%'),
    @((Get-ShortPath $env:USERPROFILE), '%USERPROFILE%')
)
[EvidenceLog.PathScrubber]::Configure([string[]]($pairs | ForEach-Object { [string]$_[0] }), [string[]]($pairs | ForEach-Object { [string]$_[1] }))
# 宿主記錄檔會寫出裸使用者名稱（wallpaper_cli 等）；電腦名稱也可能出現在系統訊息裡。
[EvidenceLog.PathScrubber]::ConfigureNames([string[]]@($env:USERNAME, $env:COMPUTERNAME), [string[]]@('%USERNAME%', '%COMPUTERNAME%'))

function ConvertTo-EvidenceText {
    <# 把字串裡的使用者設定檔路徑改寫成環境變數字樣。 #>
    param([Parameter(ValueFromPipeline)][AllowEmptyString()][AllowNull()][string]$Text)
    process { [EvidenceLog.PathScrubber]::Apply($Text) }
}

function New-EvidenceWriter {
    <# 等同 `New-Object System.IO.StreamWriter($Path, $false)`，但寫出前先改寫路徑。 #>
    param([Parameter(Mandatory)][string]$Path)
    New-Object EvidenceLog.ScrubbingWriter($Path)
}

function Protect-EvidenceFile {
    <# 就地改寫文字檔（UTF-8／UTF-16 LE，保留 BOM 與行尾）；回傳有改動的檔案數。 #>
    param([Parameter(Mandatory)][string[]]$Path)
    $changed = 0
    foreach ($p in $Path) {
        $bytes = [IO.File]::ReadAllBytes($p)
        if ($bytes.Length -ge 2 -and $bytes[0] -eq 0xFF -and $bytes[1] -eq 0xFE) {
            $enc = New-Object System.Text.UnicodeEncoding($false, $true); $pre = 2
        } elseif ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF) {
            $enc = New-Object System.Text.UTF8Encoding($true); $pre = 3
        } else {
            $enc = New-Object System.Text.UTF8Encoding($false); $pre = 0
        }
        $text = $enc.GetString($bytes, $pre, $bytes.Length - $pre)
        $new = [EvidenceLog.PathScrubber]::Apply($text)
        if ($new -ne $text) {
            $body = $enc.GetBytes($new)
            $out = New-Object byte[] ($pre + $body.Length)
            [Array]::Copy($bytes, 0, $out, 0, $pre)
            [Array]::Copy($body, 0, $out, $pre, $body.Length)
            [IO.File]::WriteAllBytes($p, $out)
            $changed++
        }
    }
    return $changed
}

function Copy-EvidenceFile {
    <# 把子行程寫的記錄檔（宿主統一記錄檔、gatekeeper.log 等）複製進證據目錄並就地去識別（fix F7）。
       來源不存在時什麼都不做（不丟例外）。 #>
    param([Parameter(Mandatory)][string]$Source, [Parameter(Mandatory)][string]$Destination)
    if (-not (Test-Path -LiteralPath $Source)) { return }
    Copy-Item -LiteralPath $Source -Destination $Destination -Force
    [void](Protect-EvidenceFile $Destination)
}

Export-ModuleMember -Function ConvertTo-EvidenceText, New-EvidenceWriter, Protect-EvidenceFile, Copy-EvidenceFile
