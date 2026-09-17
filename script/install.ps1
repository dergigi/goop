# Install the latest stable Goop release on Windows.
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'This installer requires Windows.' }
$architecture = $env:PROCESSOR_ARCHITEW6432
if (-not $architecture) { $architecture = $env:PROCESSOR_ARCHITECTURE }
switch ($architecture) {
    'AMD64' { $assetArch = 'x64' }
    'ARM64' { $assetArch = 'arm64' }
    default { throw "Unsupported Windows architecture: $architecture" }
}
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$workDir = Join-Path ([IO.Path]::GetTempPath()) ('goop-install-' + [Guid]::NewGuid())
$null = New-Item -ItemType Directory -Path $workDir
try {
    $headers = @{ 'User-Agent' = 'Goop-installer'; Accept = 'application/vnd.github+json' }
    $release = Invoke-RestMethod -Uri 'https://api.github.com/repos/dergigi/goop/releases/latest' -Headers $headers -TimeoutSec 30
    if ($release.draft -or $release.prerelease -or $release.tag_name -notmatch '^v\d+\.\d+\.\d+$') {
        throw 'Could not identify a published stable Goop release.'
    }
    $asset = "goop-windows-$assetArch.exe"
    $base = "https://github.com/dergigi/goop/releases/download/$($release.tag_name)"
    $installer = Join-Path $workDir $asset
    $checksums = Join-Path $workDir 'SHA256SUMS'
    Write-Host "Downloading Goop $($release.tag_name) ($assetArch)..."
    Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset" -OutFile $installer -TimeoutSec 600
    Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile $checksums -TimeoutSec 30
    $pattern = '^([a-fA-F0-9]{64})\s+\*?' + [regex]::Escape($asset) + '$'
    $hashes = @(Get-Content $checksums | ForEach-Object { if ($_ -match $pattern) { $Matches[1] } })
    if ($hashes.Count -ne 1) { throw 'Missing or ambiguous release checksum; installation stopped.' }
    if ((Get-FileHash -Algorithm SHA256 -Path $installer).Hash -ne $hashes[0]) {
        throw 'Release checksum mismatch; installation stopped.'
    }
    Write-Host 'Checksum verified. Follow the Goop setup prompts.'
    $process = Start-Process -FilePath $installer -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "Goop setup exited with code $($process.ExitCode)." }
    Write-Host 'Goop is installed. Open it from the Start menu.'
} finally {
    Remove-Item -LiteralPath $workDir -Recurse -Force
}
