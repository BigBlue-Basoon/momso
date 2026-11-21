# This probe uses a unique, random session credential; it never prints its value.
$ErrorActionPreference = 'Stop'
$source = @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
public static class MomsoS7Probe {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct Credential {
        public uint Flags, Type;
        public string TargetName, Comment;
        public long LastWritten;
        public uint CredentialBlobSize;
        public IntPtr CredentialBlob;
        public uint Persist, AttributeCount;
        public IntPtr Attributes;
        public string TargetAlias, UserName;
    }
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CredWriteW(ref Credential credential, uint flags);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CredReadW(string target, uint type, uint flags, out IntPtr credential);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CredDeleteW(string target, uint type, uint flags);
    [DllImport("advapi32.dll")] static extern void CredFree(IntPtr value);
    public static void Write(string target, byte[] value) {
        IntPtr blob = Marshal.AllocHGlobal(value.Length);
        try {
            Marshal.Copy(value, 0, blob, value.Length);
            var item = new Credential { Type = 1, TargetName = target, UserName = "fixture",
                CredentialBlobSize = (uint)value.Length, CredentialBlob = blob, Persist = 1 };
            if (!CredWriteW(ref item, 0)) throw new Win32Exception(Marshal.GetLastWin32Error());
        } finally {
            for (int i = 0; i < value.Length; i++) Marshal.WriteByte(blob, i, 0);
            Marshal.FreeHGlobal(blob);
        }
    }
    public static byte[] Read(string target) {
        IntPtr ptr;
        if (!CredReadW(target, 1, 0, out ptr)) throw new Win32Exception(Marshal.GetLastWin32Error());
        try {
            var item = Marshal.PtrToStructure<Credential>(ptr);
            byte[] value = new byte[item.CredentialBlobSize];
            Marshal.Copy(item.CredentialBlob, value, 0, value.Length);
            return value;
        } finally { CredFree(ptr); }
    }
    public static void Delete(string target) {
        if (!CredDeleteW(target, 1, 0)) throw new Win32Exception(Marshal.GetLastWin32Error());
    }
}
'@
Add-Type -TypeDefinition $source
$target = 'momso-s7-fixture-' + [guid]::NewGuid().ToString('N')
$secret = [byte[]]::new(32)
$random = [System.Security.Cryptography.RandomNumberGenerator]::Create()
try { $random.GetBytes($secret) } finally { $random.Dispose() }
$hash = [System.Security.Cryptography.SHA256]::Create()
try { $expected = [Convert]::ToBase64String($hash.ComputeHash($secret)) } finally { $hash.Dispose() }
$env:MOMSO_S7_PROBE_TARGET = $target
$env:MOMSO_S7_PROBE_EXPECTED = $expected
$written = $false
try {
    [MomsoS7Probe]::Write($target, $secret)
    $written = $true
    $child = @"
`$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
$source
'@
`$value = [MomsoS7Probe]::Read(`$env:MOMSO_S7_PROBE_TARGET)
try {
    `$hash = [System.Security.Cryptography.SHA256]::Create()
    try { `$actual = [Convert]::ToBase64String(`$hash.ComputeHash(`$value)) } finally { `$hash.Dispose() }
    Write-Output (`$actual -eq `$env:MOMSO_S7_PROBE_EXPECTED)
} finally { [Array]::Clear(`$value, 0, `$value.Length) }
"@
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($child))
    $start = [Diagnostics.ProcessStartInfo]::new((Get-Process -Id $PID).Path)
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in @('-NoLogo', '-NoProfile', '-NonInteractive', '-EncodedCommand', $encoded)) {
        $start.ArgumentList.Add($argument)
    }
    $process = [Diagnostics.Process]::Start($start)
    $result = $process.StandardOutput.ReadToEnd().Trim()
    $null = $process.StandardError.ReadToEnd()
    $process.WaitForExit()
    if ($process.ExitCode -ne 0 -or $result -ne 'True') { throw 'child_read_not_verified' }
    Write-Output 'same_logon_child_can_read_fixture=true'
} finally {
    if ($written) { [MomsoS7Probe]::Delete($target) }
    [Array]::Clear($secret, 0, $secret.Length)
    Remove-Item Env:MOMSO_S7_PROBE_TARGET, Env:MOMSO_S7_PROBE_EXPECTED -ErrorAction SilentlyContinue
}
