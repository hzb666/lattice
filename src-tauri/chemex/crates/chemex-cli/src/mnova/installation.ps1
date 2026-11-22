$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$path = $env:CHEMEX_MNOVA_PROBE_PATH
$registeredVersion = $null
if (-not $path) {
    $path = Get-Process -Name MestReNova -ErrorAction SilentlyContinue |
        Where-Object { [IO.File]::Exists($_.Path) } |
        Select-Object -First 1 -ExpandProperty Path
}
if (-not $path) {
    $keys = @(
        'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*',
        'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*',
        'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*'
    )
    foreach ($entry in (Get-ItemProperty -Path $keys -ErrorAction SilentlyContinue)) {
        if ($entry.DisplayName -notmatch '^MestReNova(?:\s|$)' -or -not $entry.InstallLocation) { continue }
        $candidate = Join-Path $entry.InstallLocation 'MestReNova.exe'
        if (-not [IO.File]::Exists($candidate)) { continue }
        $path = $candidate
        $registeredVersion = $entry.DisplayVersion
        break
    }
}
if (-not $path) { [Console]::WriteLine('null'); exit }
$info = [Diagnostics.FileVersionInfo]::GetVersionInfo($path)
$version = @($info.FileVersion, $info.ProductVersion, $registeredVersion) |
    Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | Select-Object -First 1
[Console]::WriteLine((ConvertTo-Json -InputObject @($path, $version) -Compress))
