# Run by install.cmd after Silicon Apps installs or updates the ring CLI on Windows x64, also from
# the background updater.
#
# The CLI is for silicons and is already in place. This adds the Ring desktop app for the people
# (carbons) who use this Windows account, without administrator rights: the self-contained build in
# this package goes to %LOCALAPPDATA%\Programs\Ring\app-<build>, with a Ring shortcut in the Start
# menu. Each build gets its own folder, so a Ring that is running (it stays in the notification
# area) never blocks an update: the Start menu shortcut, taskbar pins, desktop shortcuts and "open
# at login" move to the new build, and earlier builds are removed once nothing runs them. An update
# that brings the same build changes nothing.
#
# It does nothing on Windows older than 10 version 1809, on a machine without a desktop (Server
# Core), and it gives way to the MSI (Ring for everyone): shortcuts and login move to the MSI's Ring
# at once, and this user's copy is removed once nothing runs it. To turn it off for good: run any
# install or update once with RING_NO_DESKTOP=1, or create the file
# %LOCALAPPDATA%\Programs\Ring.settings\no-desktop (delete it to turn it back on). To remove the app
# as well, also delete %LOCALAPPDATA%\Programs\Ring and the Ring shortcut in the Start menu.
#
# It never fails the CLI install, and it stops well inside Silicon Apps' time limit.

$ErrorActionPreference = 'Stop'
$started = Get-Date

function Say([string] $message) { Write-Output "ring desktop app: $message" }

# The background updater starts this in a console window of its own: hide it, so nobody closes it
# (which would stop the update). A console shared with a terminal (an install typed there) stays.
try {
    Add-Type -Namespace RingSetup -Name Console -MemberDefinition @'
[DllImport("kernel32.dll")] public static extern System.IntPtr GetConsoleWindow();
[DllImport("kernel32.dll")] public static extern uint GetConsoleProcessList(uint[] list, uint count);
[DllImport("user32.dll")] public static extern bool ShowWindow(System.IntPtr window, int show);
'@
    $attached = New-Object 'uint32[]' 16
    if ([RingSetup.Console]::GetConsoleProcessList($attached, 16) -le 2) {
        [void][RingSetup.Console]::ShowWindow([RingSetup.Console]::GetConsoleWindow(), 0)
    }
} catch { }

function Get-RingProcess([string] $folder) {
    Get-Process -Name 'Ring' -ErrorAction SilentlyContinue | Where-Object {
        try { $_.Path -and $_.Path.StartsWith($folder + '\', [StringComparison]::OrdinalIgnoreCase) } catch { $false }
    }
}

# The same AppUserModelID as the app (Program.cs) and the MSI's shortcut, so a pinned Ring and its
# window are one taskbar button and notifications name Ring.
function Set-AppUserModelId([string] $shortcut, [string] $appId) {
    if (-not ('RingSetup.Shortcut' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace RingSetup {
    public static class Shortcut {
        [ComImport, Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        interface IPropertyStore {
            [PreserveSig] int GetCount(out uint count);
            [PreserveSig] int GetAt(uint index, out PropertyKey key);
            [PreserveSig] int GetValue(ref PropertyKey key, out PropVariant value);
            [PreserveSig] int SetValue(ref PropertyKey key, ref PropVariant value);
            [PreserveSig] int Commit();
        }
        [StructLayout(LayoutKind.Sequential, Pack = 4)]
        struct PropertyKey { public Guid FormatId; public uint PropertyId; }
        [StructLayout(LayoutKind.Explicit, Size = 24)]
        struct PropVariant { [FieldOffset(0)] public ushort Type; [FieldOffset(8)] public IntPtr Pointer; }
        [DllImport("shell32.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
        static extern void SHGetPropertyStoreFromParsingName(string path, IntPtr bindContext, int flags, ref Guid iid,
            [MarshalAs(UnmanagedType.Interface)] out IPropertyStore store);
        public static void SetAppId(string path, string appId) {
            Guid iid = typeof(IPropertyStore).GUID;
            IPropertyStore store;
            SHGetPropertyStoreFromParsingName(path, IntPtr.Zero, 2, ref iid, out store);
            PropertyKey key = new PropertyKey { FormatId = new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"), PropertyId = 5 };
            PropVariant value = new PropVariant { Type = 31, Pointer = Marshal.StringToCoTaskMemUni(appId) };
            try {
                Marshal.ThrowExceptionForHR(store.SetValue(ref key, ref value));
                Marshal.ThrowExceptionForHR(store.Commit());
            } finally {
                Marshal.FreeCoTaskMem(value.Pointer);
                Marshal.ReleaseComObject(store);
            }
        }
    }
}
'@
    }
    [RingSetup.Shortcut]::SetAppId($shortcut, $appId)
}

# Taskbar pins and desktop copies of the Start shortcut keep their own target: move the ones that
# start this user's copy to $target before a build (or the whole copy) is removed.
function Update-RingShortcuts([string] $target) {
    $pinned = Join-Path $env:APPDATA 'Microsoft\Internet Explorer\Quick Launch\User Pinned'
    # Desktop copies of the shortcut sit at its top level; the Desktop itself can hold anything.
    $desktop = [Environment]::GetFolderPath('Desktop')
    foreach ($place in @((Join-Path $pinned 'TaskBar'), (Join-Path $pinned 'ImplicitAppShortcuts'), $desktop)) {
        if (-not $place -or -not (Test-Path -LiteralPath $place)) { continue }
        Get-ChildItem -LiteralPath $place -Filter '*.lnk' -Recurse:($place -ne $desktop) -Force -ErrorAction SilentlyContinue | ForEach-Object {
            try {
                $copy = $shell.CreateShortcut($_.FullName)
                if ($copy.TargetPath -and $copy.TargetPath.StartsWith($app + '\', [StringComparison]::OrdinalIgnoreCase) -and
                    -not $copy.TargetPath.Equals($target, [StringComparison]::OrdinalIgnoreCase)) {
                    $folder = Split-Path -Parent $target
                    $copy.TargetPath = $target
                    $copy.WorkingDirectory = $folder
                    $copy.IconLocation = (Join-Path $folder 'Assets\Ring.ico') + ',0'
                    $copy.Save()
                    try { Set-AppUserModelId $_.FullName 'TeamOfSilicons.Ring' } catch { }
                }
            } catch { }
        }
    }
}

try {
    $programs = Join-Path $env:LOCALAPPDATA 'Programs'
    $app = Join-Path $programs 'Ring'
    $settings = Join-Path $programs 'Ring.settings'
    $optOut = Join-Path $settings 'no-desktop'
    # The opt-out is a file, so the background updater sees it too whatever its environment.
    if ($env:RING_NO_DESKTOP) {
        try {
            New-Item -ItemType Directory -Force -Path $settings | Out-Null
            New-Item -ItemType File -Force -Path $optOut | Out-Null
        } catch { }
    }
    $turnedOff = $env:RING_NO_DESKTOP -or (Test-Path -LiteralPath $optOut)
    if ([Environment]::OSVersion.Version.Build -lt 17763) {
        Say 'skipped: Ring needs Windows 10 version 1809 or later'
        exit 0
    }
    if (-not (Test-Path -LiteralPath (Join-Path $env:WINDIR 'explorer.exe'))) {
        Say 'skipped: this machine has no desktop'
        exit 0
    }

    $runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
    $lnk = Join-Path ([Environment]::GetFolderPath('Programs')) 'Ring.lnk'
    $shell = New-Object -ComObject WScript.Shell
    $login = (Get-ItemProperty -Path $runKey -Name 'Ring' -ErrorAction SilentlyContinue).Ring
    # Ours: it starts one of this user's builds, or a Ring.exe that no longer exists (an MSI since
    # removed). An empty value means the person turned "open at login" off; that stays.
    $loginIsOurs = $login -and (
        $login.IndexOf($app + '\', [StringComparison]::OrdinalIgnoreCase) -ge 0 -or
        ($login -match '^\s*"([^"]+\\Ring\.exe)"' -and -not (Test-Path -LiteralPath $Matches[1])))

    $machine = @($env:ProgramFiles, ${env:ProgramFiles(x86)}) | Where-Object { $_ } |
        ForEach-Object { Join-Path $_ 'Ring\Ring.exe' } | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
    if ($machine) {
        # The MSI provides Ring for everyone; this user's own copy would only compete with it. Login,
        # the Start menu and pins move to the MSI's Ring now; the folder goes once nothing runs it.
        if ($loginIsOurs) { Set-ItemProperty -Path $runKey -Name 'Ring' -Value ('"' + $machine + '" --background') }
        if ((Test-Path -LiteralPath $lnk) -and
            $shell.CreateShortcut($lnk).TargetPath.StartsWith($app + '\', [StringComparison]::OrdinalIgnoreCase)) {
            Remove-Item -LiteralPath $lnk -Force
        }
        if (Test-Path -LiteralPath $app) {
            Update-RingShortcuts $machine
            if (-not (Get-RingProcess $app)) {
                Remove-Item -LiteralPath $app -Recurse -Force -ErrorAction SilentlyContinue
                Say "removed this user's own copy in favour of the MSI"
            }
        }
        Say 'skipped: Ring is installed for everyone on this machine (MSI)'
        exit 0
    }
    if ($turnedOff) {
        Say "skipped: turned off for this user (delete $optOut to turn it back on)"
        exit 0
    }

    # The client passes an extended-length path (\\?\C:\... or \\?\UNC\server\...), which Windows
    # PowerShell 5.1's path cmdlets cannot use.
    $package = $env:APPS_INSTALL_DIR
    if ($package.StartsWith('\\?\UNC\')) { $package = '\\' + $package.Substring(8) }
    elseif ($package.StartsWith('\\?\')) { $package = $package.Substring(4) }
    $zip = Join-Path $package 'desktop\Ring-windows-x64.zip'
    if (-not (Test-Path -LiteralPath $zip)) { throw "$zip is missing from the package" }
    $hash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.Substring(0, 16).ToLowerInvariant()
    $build = Join-Path $app "app-$hash"
    $exe = Join-Path $build 'Ring.exe'
    New-Item -ItemType Directory -Force -Path $app | Out-Null
    # What earlier runs left when the install script's time limit stopped them.
    Get-ChildItem -LiteralPath $app -Directory -Force -Filter '.install-*' -ErrorAction SilentlyContinue |
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue

    if (-not (Test-Path -LiteralPath $exe)) {
        $work = Join-Path $app ('.install-' + [guid]::NewGuid().ToString('N'))
        try {
            Add-Type -AssemblyName System.IO.Compression
            Add-Type -AssemblyName System.IO.Compression.FileSystem
            $root = [System.IO.Path]::GetFullPath($work) + '\'
            $archive = [System.IO.Compression.ZipFile]::OpenRead($zip)
            try {
                foreach ($entry in $archive.Entries) {
                    # Silicon Apps stops the script at 120 s and rolls the CLI back: stay well inside it.
                    if (((Get-Date) - $started).TotalSeconds -gt 75) {
                        throw 'unpacking Ring takes too long on this machine right now; the next update tries again'
                    }
                    $target = [System.IO.Path]::GetFullPath([System.IO.Path]::Combine($work, $entry.FullName))
                    if (-not $target.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) { throw "unexpected path $($entry.FullName) in the package" }
                    if ($entry.FullName.EndsWith('/')) { [void][System.IO.Directory]::CreateDirectory($target); continue }
                    [void][System.IO.Directory]::CreateDirectory([System.IO.Path]::GetDirectoryName($target))
                    [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $target, $true)
                }
            } finally {
                $archive.Dispose()
            }
            $new = Join-Path $work 'Ring'
            if (-not (Test-Path -LiteralPath (Join-Path $new 'Ring.exe'))) { throw 'the package has no Ring\Ring.exe' }
            if (Test-Path -LiteralPath $build) { Remove-Item -LiteralPath $build -Recurse -Force }
            # Antivirus may still hold the files just written, which blocks the rename for a moment.
            for ($attempt = 1; ; $attempt++) {
                try { Move-Item -LiteralPath $new -Destination $build; break }
                catch { if ($attempt -ge 20) { throw }; Start-Sleep -Milliseconds 500 }
            }
        } finally {
            if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue }
        }
    }

    $shortcut = $shell.CreateShortcut($lnk)
    $shortcut.TargetPath = $exe
    $shortcut.WorkingDirectory = $build
    $shortcut.IconLocation = (Join-Path $build 'Assets\Ring.ico') + ',0'
    $shortcut.Description = 'Calls between carbons and silicons'
    $shortcut.Save()
    try { Set-AppUserModelId $lnk 'TeamOfSilicons.Ring' } catch { Say "the shortcut has no AppUserModelID ($($_.Exception.Message))" }
    Update-RingShortcuts $exe
    # "Open at login" follows the new build when it started one of this user's copies.
    if ($loginIsOurs) { Set-ItemProperty -Path $runKey -Name 'Ring' -Value ('"' + $exe + '" --background') }

    # Earlier builds go once nothing runs them; one still running goes with a later update.
    $running = @(Get-RingProcess $app | ForEach-Object { Split-Path -Parent $_.Path })
    Get-ChildItem -LiteralPath $app -Directory -Filter 'app-*' | Where-Object {
        $_.FullName -ne $build -and -not ($running -contains $_.FullName)
    } | ForEach-Object { Remove-Item -LiteralPath $_.FullName -Recurse -Force -ErrorAction SilentlyContinue }

    Say "installed for $env:USERNAME in $build; open Ring from the Start menu"
} catch {
    Say "not installed this time ($($_.Exception.Message)); the ring CLI is installed and works without it"
}
exit 0
