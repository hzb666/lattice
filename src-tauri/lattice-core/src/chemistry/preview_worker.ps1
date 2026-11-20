$ErrorActionPreference = 'Stop'
$app = $null
$document = $null
$revision = ''
$source = $null
$directory = $PSScriptRoot
$output = Join-Path $directory 'group.svg'
$inputReader = [IO.StreamReader]::new([Console]::OpenStandardInput(), [Text.Encoding]::UTF8)
Add-Type -AssemblyName System.Windows.Forms

function Close-PrivateDocument {
    if ($null -eq $script:document) { return }
    $save = $false
    $name = [Type]::Missing
    $closing = $script:document
    $script:document = $null
    try { $closing.Close([ref]$save, [ref]$name) }
    finally { [void][Runtime.InteropServices.Marshal]::ReleaseComObject($closing) }
}

function Find-Group($groups, [int]$groupId) {
    for ($index = 1; $index -le $groups.Count; $index++) {
        $group = $groups.Item($index)
        if ($group.ID -eq $groupId) { return ,$group }
        $nested = Find-Group $group.Groups $groupId
        if ($null -ne $nested) { return ,$nested }
    }
}

function Export-Group([string]$requested, [int]$groupId) {
    if ($null -eq $script:app) { $script:app = New-Object -ComObject ChemDraw_x64.Application }
    if ($script:revision -ne $requested -or $null -eq $script:document) {
        Close-PrivateDocument
        $script:source = Join-Path $directory "$requested.cdxml"
        $password = $format = [Type]::Missing
        $script:document = $script:app.Documents.Open($script:source, [ref]$password, [ref]$format)
        $script:revision = $requested
    }
    $group = Find-Group $script:document.Groups $groupId
    if ($null -eq $group) { throw "ChemDraw group $groupId was not found" }
    $missing = [Type]::Missing
    [byte[]]$svg = $group.Objects.Data('image/svg+xml', $missing, $missing, $missing)
    if (!$svg.Length -or $svg.Length -gt 64MB) { throw 'Invalid group SVG size' }
    [IO.File]::WriteAllBytes($output, $svg)
}

try {
    while ($true) {
        $pending = $inputReader.ReadLineAsync()
        while (!$pending.IsCompleted) {
            [Windows.Forms.Application]::DoEvents()
            Start-Sleep -Milliseconds 10
        }
        $line = $pending.GetAwaiter().GetResult()
        if ($null -eq $line) { break }
        try {
            if ($line -cnotmatch '^([0-9a-f]{64}) (-?[0-9]{1,10})$') { throw 'Invalid export request' }
            Export-Group $Matches[1] ([int]$Matches[2])
            [Console]::WriteLine('{}')
        } catch {
            $reply = @{ error = $_.Exception.Message } | ConvertTo-Json -Compress
            [Console]::WriteLine($reply)
        }
    }
} finally {
    try { Close-PrivateDocument } finally {
        if ($null -ne $app) { [void][Runtime.InteropServices.Marshal]::ReleaseComObject($app) }
        if ($null -ne $source) { [IO.File]::Delete($source) }
        [IO.File]::Delete($output)
        [IO.File]::Delete($PSCommandPath)
        [IO.Directory]::Delete($directory)
    }
}
