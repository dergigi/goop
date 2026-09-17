$ErrorActionPreference = 'Stop'
$installerScript = Join-Path $PSScriptRoot 'install.ps1'
function Invoke-RestMethod {
    param($Uri, $Headers, $TimeoutSec)
    if ($global:goopTest_scenario -eq 'download') { throw 'Download failed' }
    return @{ tag_name = 'v2.11.1'; draft = $false; prerelease = ($global:goopTest_scenario -eq 'prerelease') }
}
function Invoke-WebRequest {
    param([switch]$UseBasicParsing, $Uri, $OutFile, $TimeoutSec)
    $global:goopTest_downloadFolder = Split-Path $OutFile
    if ($Uri.EndsWith('SHA256SUMS')) {
        $hash = (Get-FileHash (Join-Path $global:goopTest_downloadFolder "goop-windows-$global:goopTest_expectedArch.exe")).Hash
        if ($global:goopTest_scenario -eq 'checksum') { $hash = '0' * 64 }
        $line = "$hash  goop-windows-$global:goopTest_expectedArch.exe"
        if ($global:goopTest_scenario -eq 'duplicate') { $line += "`n$line" }
        Set-Content -Path $OutFile -Value $line
    } else { Set-Content -Path $OutFile -Value 'mock installer' }
}
function Start-Process {
    param($FilePath, [switch]$Wait, [switch]$PassThru)
    $global:goopTest_launched = $true
    if (-not $FilePath.EndsWith("goop-windows-$global:goopTest_expectedArch.exe")) { throw 'Wrong architecture' }
    return @{ ExitCode = $(if ($global:goopTest_scenario -eq 'setup') { 1 } else { 0 }) }
}
foreach ($architecture in @('AMD64', 'ARM64')) {
    foreach ($case in @('success', 'download', 'checksum', 'duplicate', 'prerelease', 'setup')) {
        $global:goopTest_scenario = $case
        $global:goopTest_expectedArch = $(if ($architecture -eq 'ARM64') { 'arm64' } else { 'x64' })
        $env:PROCESSOR_ARCHITECTURE = $architecture
        $env:PROCESSOR_ARCHITEW6432 = ''
        $global:goopTest_launched = $false
        $global:goopTest_downloadFolder = $null
        $failed = $false
        try { & $installerScript } catch { $failed = $true; Write-Host $_.Exception.Message }
        if ($failed -ne ($case -ne 'success')) { throw "Unexpected result: $architecture / $case" }
        if ($global:goopTest_launched -ne ($case -in @('success', 'setup'))) { throw "Unexpected launch: $case" }
        if ($global:goopTest_downloadFolder -and (Test-Path $global:goopTest_downloadFolder)) { throw 'Temporary files remain' }
    }
}
Write-Host '12 mocked Windows installer checks passed.'
