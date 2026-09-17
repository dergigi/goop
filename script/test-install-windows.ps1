$ErrorActionPreference = 'Stop'
$installerScript = Join-Path $PSScriptRoot 'install.ps1'
function Invoke-RestMethod {
    param($Uri, $Headers, $TimeoutSec)
    if ($script:scenario -eq 'download') { throw 'Download failed' }
    return @{ tag_name = 'v2.11.1'; draft = $false; prerelease = ($script:scenario -eq 'prerelease') }
}
function Invoke-WebRequest {
    param([switch]$UseBasicParsing, $Uri, $OutFile, $TimeoutSec)
    $script:downloadFolder = Split-Path $OutFile
    if ($Uri.EndsWith('SHA256SUMS')) {
        $hash = (Get-FileHash (Join-Path $script:downloadFolder "goop-windows-$script:expectedArch.exe")).Hash
        if ($script:scenario -eq 'checksum') { $hash = '0' * 64 }
        $line = "$hash  goop-windows-$script:expectedArch.exe"
        if ($script:scenario -eq 'duplicate') { $line += "`n$line" }
        Set-Content -Path $OutFile -Value $line
    } else { Set-Content -Path $OutFile -Value 'mock installer' }
}
function Start-Process {
    param($FilePath, [switch]$Wait, [switch]$PassThru)
    $script:launched = $true
    if (-not $FilePath.EndsWith("goop-windows-$script:expectedArch.exe")) { throw 'Wrong architecture' }
    return @{ ExitCode = $(if ($script:scenario -eq 'setup') { 1 } else { 0 }) }
}
foreach ($architecture in @('AMD64', 'ARM64')) {
    foreach ($case in @('success', 'download', 'checksum', 'duplicate', 'prerelease', 'setup')) {
        $script:scenario = $case
        $script:expectedArch = $(if ($architecture -eq 'ARM64') { 'arm64' } else { 'x64' })
        $env:PROCESSOR_ARCHITECTURE = $architecture
        $env:PROCESSOR_ARCHITEW6432 = ''
        $script:launched = $false
        $script:downloadFolder = $null
        $failed = $false
        try { & $installerScript } catch { $failed = $true }
        if ($failed -ne ($case -ne 'success')) { throw "Unexpected result: $architecture / $case" }
        if ($script:launched -ne ($case -in @('success', 'setup'))) { throw "Unexpected launch: $case" }
        if ($script:downloadFolder -and (Test-Path $script:downloadFolder)) { throw 'Temporary files remain' }
    }
}
Write-Host '12 mocked Windows installer checks passed.'
