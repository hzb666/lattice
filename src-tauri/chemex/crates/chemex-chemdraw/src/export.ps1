$ErrorActionPreference = 'Stop'
$app = $null
$document = $null
try {
    $source = [IO.Path]::GetFullPath($env:CHEMEX_EXPORT_SOURCE)
    $destination = [IO.Path]::GetFullPath($env:CHEMEX_EXPORT_SVG)
    if (!(Test-Path -LiteralPath $source -PathType Leaf)) { throw 'Missing private ChemDraw input copy' }
    if (Test-Path -LiteralPath $destination) { throw 'SVG destination already exists' }
    $app = New-Object -ComObject ChemDraw_x64.Application
    for ($index = 1; $index -le $app.Documents.Count; $index++) {
        $existing = $app.Documents.Item($index)
        try {
            if ($existing.FullName -eq $source) { throw 'Private input is already open; refusing to reuse it' }
        } finally {
            [void][Runtime.InteropServices.Marshal]::ReleaseComObject($existing)
        }
    }
    $supported = $false
    $types = $app.ExportDataTypes
    for ($index = 1; $index -le $types.Count; $index++) {
        $type = $types.Item($index)
        if ($type.Valid -and $type.MIME -eq 'image/svg+xml') { $supported = $true }
        [void][Runtime.InteropServices.Marshal]::ReleaseComObject($type)
    }
    [void][Runtime.InteropServices.Marshal]::ReleaseComObject($types)
    if (!$supported) { throw 'This ChemDraw installation does not advertise SVG export' }
    $password = [Type]::Missing
    $format = [Type]::Missing
    $document = $app.Documents.Open($source, [ref]$password, [ref]$format)
    $format = 'image/svg+xml'
    $resolution = [Type]::Missing
    $width = [Type]::Missing
    $height = [Type]::Missing
    $document.SaveAs([ref]$destination, [ref]$format, [ref]$resolution, [ref]$width, [ref]$height)
    if (!(Test-Path -LiteralPath $destination -PathType Leaf)) { throw 'ChemDraw returned without an SVG' }
    [pscustomobject]@{ application = $app.FullName; format = $format; output = $destination } | ConvertTo-Json
} catch {
    [Console]::Error.WriteLine($_.Exception.ToString())
    exit 1
} finally {
    if ($null -ne $document) {
        $save = $false
        $name = [Type]::Missing
        $document.Close([ref]$save, [ref]$name)
        [void][Runtime.InteropServices.Marshal]::ReleaseComObject($document)
    }
    if ($null -ne $app) { [void][Runtime.InteropServices.Marshal]::ReleaseComObject($app) }
}
