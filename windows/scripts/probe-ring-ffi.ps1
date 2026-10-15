<#
.SYNOPSIS
    Loads ring_ffi.dll and exercises its C ABI (windows/native/ring_ffi.h) without a call.

.DESCRIPTION
    Checks the exported version, that invalid arguments are refused without crashing, and
    (with -Devices) that audio devices can be listed through LiveKit's platform audio.
    Run it with the PowerShell that matches the DLL's architecture.

.EXAMPLE
    pwsh windows/scripts/probe-ring-ffi.ps1 -Dll target\x86_64-pc-windows-msvc\release\ring_ffi.dll -ExpectVersion 1.0.0
#>
param(
    [Parameter(Mandatory = $true)][string]$Dll,
    [string]$ExpectVersion,
    [switch]$Devices
)

$ErrorActionPreference = 'Stop'
$path = (Resolve-Path $Dll).Path
$escaped = $path.Replace('\', '\\')

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;

public static class RingFfiProbe
{
    [DllImport("$escaped", EntryPoint = "ring_ffi_version")] public static extern IntPtr Version();
    [DllImport("$escaped", EntryPoint = "ring_ffi_last_error")] public static extern IntPtr LastError();
    [DllImport("$escaped", EntryPoint = "ring_string_free")] public static extern void StringFree(IntPtr s);
    [DllImport("$escaped", EntryPoint = "ring_media_input_devices_json")] public static extern IntPtr InputDevices();
    [DllImport("$escaped", EntryPoint = "ring_media_output_devices_json")] public static extern IntPtr OutputDevices();
    [DllImport("$escaped", EntryPoint = "ring_media_connect")] public static extern IntPtr Connect(IntPtr url, IntPtr token, IntPtr options, IntPtr callback, IntPtr ctx);
    [DllImport("$escaped", EntryPoint = "ring_media_set_muted")] public static extern int SetMuted(IntPtr media, int muted);
    [DllImport("$escaped", EntryPoint = "ring_media_is_muted")] public static extern int IsMuted(IntPtr media);
    [DllImport("$escaped", EntryPoint = "ring_media_disconnect")] public static extern void Disconnect(IntPtr media);

    public static string Take(IntPtr p)
    {
        if (p == IntPtr.Zero) return null;
        try { return Marshal.PtrToStringUTF8(p); } finally { StringFree(p); }
    }
}
"@

$version = [System.Runtime.InteropServices.Marshal]::PtrToStringUTF8([RingFfiProbe]::Version())
Write-Host "ring_ffi_version: $version"
if ($ExpectVersion -and $version -ne $ExpectVersion) { throw "expected version $ExpectVersion, got '$version'" }

$handle = [RingFfiProbe]::Connect([IntPtr]::Zero, [IntPtr]::Zero, [IntPtr]::Zero, [IntPtr]::Zero, [IntPtr]::Zero)
if ($handle -ne [IntPtr]::Zero) { throw 'ring_media_connect accepted null arguments' }
Write-Host "ring_media_connect(NULL) refused: $([RingFfiProbe]::Take([RingFfiProbe]::LastError()))"

if ([RingFfiProbe]::SetMuted([IntPtr]::Zero, 1) -ne -1) { throw 'ring_media_set_muted(NULL) did not return RING_ERR_INVALID_ARGUMENT' }
if ([RingFfiProbe]::IsMuted([IntPtr]::Zero) -ne -1) { throw 'ring_media_is_muted(NULL) did not return RING_ERR_INVALID_ARGUMENT' }
[RingFfiProbe]::Disconnect([IntPtr]::Zero)

if ($Devices) {
    Write-Host "input devices:  $([RingFfiProbe]::Take([RingFfiProbe]::InputDevices()))"
    Write-Host "output devices: $([RingFfiProbe]::Take([RingFfiProbe]::OutputDevices()))"
}

Write-Host 'ring_ffi C ABI probe passed'
