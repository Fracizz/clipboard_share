$ErrorActionPreference = "Stop"

$projectRoot = $PSScriptRoot
$releaseExe = Join-Path $projectRoot "target\release\clipboard_share.exe"
$distRoot = Join-Path $projectRoot "dist"

Push-Location $projectRoot
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build --release failed with exit code $LASTEXITCODE"
    }

    if (Test-Path $distRoot) {
        Remove-Item $distRoot -Recurse -Force
    }

    foreach ($side in @("A", "B")) {
        $sideDir = Join-Path $distRoot "ClipboardShare-$side"
        New-Item -ItemType Directory -Path $sideDir -Force | Out-Null
        Copy-Item $releaseExe (Join-Path $sideDir "clipboard_share.exe")
        Copy-Item (Join-Path $projectRoot "portable\$side\config.json") $sideDir
        Copy-Item (Join-Path $projectRoot "portable\start.bat") $sideDir
        Copy-Item (Join-Path $projectRoot "portable\stop.bat") $sideDir
        Copy-Item (Join-Path $projectRoot "README.md") $sideDir
        Compress-Archive `
            -Path (Join-Path $sideDir "*") `
            -DestinationPath (Join-Path $distRoot "ClipboardShare-$side.zip") `
            -CompressionLevel Optimal
    }

    Write-Host "Portable packages created:"
    Write-Host "  $distRoot\ClipboardShare-A.zip"
    Write-Host "  $distRoot\ClipboardShare-B.zip"
}
finally {
    Pop-Location
}
