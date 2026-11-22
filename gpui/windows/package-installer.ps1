<#
.SYNOPSIS
Packages an existing x64 release with the native Agent guard. Run on a trusted release worker.
.DESCRIPTION
Stage bin/lattice-gpui.exe from a validated build with windows-agent-guard enabled.
Use the signed-driver --lattice-guard-probe smoke result for that EXE and driver.
The default GPUI CI EXE is not suitable. This script does not build or run the app.
Stage bin/pdfium.dll, gpui/islands/dist as Resources/islands, and the Windows
src-tauri/synara-runtime output as Resources/synara-runtime. Include any bundled
skills at Resources/src/embedded_skills. Stage the Microsoft-signed
LatticeAgentGuard INF/CAT/SYS under driver. Keep the stage immutable during packaging.
Authenticode-sign the resulting setup executable before distribution.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Stage,
    [Parameter(Mandatory)][string]$SignTool,
    [Parameter(Mandatory)][string]$MakeNsis,
    [Parameter(Mandatory)][string]$OutputPath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Get-StageEntries([IO.DirectoryInfo]$Directory) {
    foreach ($entry in $Directory.EnumerateFileSystemInfos()) {
        if ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "Reparse point in installer stage: $($entry.FullName)"
        }
        $entry
        if ($entry -is [IO.DirectoryInfo]) {
            Get-StageEntries $entry
        }
    }
}

function ConvertTo-NsisLiteral([string]$Text, [switch]$CompileTime) {
    if ($Text -match '[`\r\n"]|\$\{|\$%') {
        throw "Path cannot be emitted as an NSIS literal: $Text"
    }
    if ($CompileTime) {
        return $Text
    }
    $Text.Replace('$', '$$')
}

function Assert-Signature([string]$Path) {
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne 'Valid') {
        throw "Authenticode verification failed ($($signature.Status)): $Path"
    }
    $signature.SignerCertificate
}

$root = Get-Item -LiteralPath $Stage
if ($root -isnot [IO.DirectoryInfo] -or $root.FullName -notmatch '^[A-Za-z]:\\') {
    throw 'The installer stage must be a local directory.'
}
for ($ancestor = $root; $ancestor; $ancestor = $ancestor.Parent) {
    if ($ancestor.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "Reparse point on installer stage path: $($ancestor.FullName)"
    }
}
$prefix = $root.FullName.TrimEnd('\') + '\'
$output = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputPath)
if ($output.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase) -or
    [IO.Path]::GetExtension($output) -ne '.exe' -or (Test-Path -LiteralPath $output)) {
    throw 'Choose a new setup .exe path outside the installer stage.'
}

$entries = @(Get-StageEntries $root | Sort-Object FullName)
$files = @{}
$directories = @()
foreach ($entry in $entries) {
    $relative = $entry.FullName.Substring($prefix.Length)
    if ($relative -notmatch '^(bin|Resources|driver)(\\|$)') {
        throw "Unexpected installer stage entry: $relative"
    }
    if ($entry -is [IO.DirectoryInfo]) {
        $directories += $relative
    } else {
        $files.Add($relative, $entry)
    }
}

$driver = 'driver\LatticeAgentGuard'
$runtimeManifest = 'Resources\synara-runtime\manifest.json'
$required = @(
    'bin\lattice-gpui.exe', 'bin\pdfium.dll',
    'Resources\islands\sheet\index.html', 'Resources\islands\agent\index.html',
    'Resources\synara-runtime\bin\node.exe', $runtimeManifest,
    'Resources\synara-runtime\server\dist\index.mjs',
    'Resources\synara-runtime\server\dist\client\index.html',
    "$driver.inf", "$driver.cat", "$driver.sys"
)
foreach ($relative in $required) {
    if (!$files.ContainsKey($relative) -or !$files[$relative].Length) {
        throw "Missing or empty staged file: $relative"
    }
}
if (@($entries | Where-Object { $_.FullName.Substring($prefix.Length) -like 'driver\*' }).Count -ne 3) {
    throw 'The driver stage must contain exactly the INF, CAT and SYS.'
}

$runtimePin = Get-Content -Raw -LiteralPath "$PSScriptRoot\..\..\scripts\synara-runtime.json" | ConvertFrom-Json
$runtime = Get-Content -Raw -LiteralPath $files[$runtimeManifest].FullName | ConvertFrom-Json
if ($runtime.target -cne 'x86_64-pc-windows-msvc' -or $runtime.nodeRuntime -cne 'standalone' -or
    $runtime.nodeVersion -cne $runtimePin.nodeVersion -or $runtime.synaraRevision -cne $runtimePin.revision) {
    throw 'Stage the pinned x64 Windows Synara and standalone Node runtime for this revision.'
}

$template = (Get-Content -Raw -LiteralPath "$PSScriptRoot\agent-guard\LatticeAgentGuard.inf.in").Replace("`r`n", "`n").Trim()
$pattern = [regex]::Escape($template).
    Replace('@DRIVER_DATE@', '[0-9]{2}/[0-9]{2}/[0-9]{4}').
    Replace('@DRIVER_VERSION@', '[0-9]+(?:\.[0-9]+){3}').
    Replace('@MICROSOFT_ASSIGNED_ALTITUDE@', '[1-9][0-9]*(?:\.[0-9]+)?')
$inf = (Get-Content -Raw -LiteralPath $files["$driver.inf"].FullName).Replace("`r`n", "`n").Trim()
if ($inf -cnotmatch "\A$pattern\z") {
    throw 'Stage the release INF generated from LatticeAgentGuard.inf.in with the assigned altitude and release DriverVer.'
}

$catalog = $files["$driver.cat"].FullName
$signer = Assert-Signature $catalog
if ($signer.GetNameInfo([Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false) -cne
    'Microsoft Windows Hardware Compatibility Publisher') {
    throw 'The driver catalog requires the Microsoft production publisher.'
}
$signToolExe = (Get-Command $SignTool -CommandType Application).Source
foreach ($member in @("$driver.inf", "$driver.sys")) {
    & $signToolExe verify /kp /c $catalog $files[$member].FullName
    if ($LASTEXITCODE -ne 0) {
        throw "Kernel-policy catalog verification failed: $member"
    }
}
$includePath = [IO.Path]::GetTempFileName()
try {
    $lines = @(
        '!macro InstallPayload'
        foreach ($relative in $directories) {
            '  SetOutPath "$INSTDIR\{0}"' -f (ConvertTo-NsisLiteral $relative)
        }
        foreach ($relative in ($files.Keys | Sort-Object)) {
            '  File "/oname=$INSTDIR\{0}" "{1}"' -f
                (ConvertTo-NsisLiteral $relative), (ConvertTo-NsisLiteral $files[$relative].FullName -CompileTime)
        }
        '!macroend'
        '!macro CheckPayloadDirectories'
        foreach ($relative in $directories) {
            '  !insertmacro CheckDirectory "$INSTDIR\{0}"' -f (ConvertTo-NsisLiteral $relative)
        }
        '!macroend'
        '!macro RemovePayloadFiles'
        foreach ($relative in $files.Keys) {
            $path = ConvertTo-NsisLiteral $relative
            '  IfFileExists "$INSTDIR\{0}" 0 +2' -f $path
            '  Delete "$INSTDIR\{0}"' -f $path
        }
        '!macroend'
        '!macro RemovePayloadDirectories'
        foreach ($relative in ($directories | Sort-Object Length -Descending)) {
            '  RMDir "$INSTDIR\{0}"' -f (ConvertTo-NsisLiteral $relative)
        }
        '!macroend'
    )
    [IO.File]::WriteAllLines($includePath, $lines, [Text.UTF8Encoding]::new($true))
    $makeNsisExe = (Get-Command $MakeNsis -CommandType Application).Source
    $includeArgument = ConvertTo-NsisLiteral $includePath -CompileTime
    $outputArgument = ConvertTo-NsisLiteral $output -CompileTime
    & $makeNsisExe /INPUTCHARSET UTF8 "/DPAYLOAD_INCLUDE=$includeArgument" "/DSETUP_OUT=$outputArgument" "$PSScriptRoot\lattice.nsi"
    if ($LASTEXITCODE -ne 0) {
        throw "makensis failed with exit code $LASTEXITCODE."
    }
} finally {
    Remove-Item -LiteralPath $includePath
}
