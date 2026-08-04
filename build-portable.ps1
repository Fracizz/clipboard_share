$ErrorActionPreference = "Stop"

$projectRoot = $PSScriptRoot
$releaseExe = Join-Path $projectRoot "target\release\clipboard_share.exe"
$uiExe = Join-Path $projectRoot "target\release\clipboard_share_ui.exe"
# Distribution artifacts only (not cargo target/). Always under packages/.
$distRoot = Join-Path $projectRoot "packages"

Push-Location $projectRoot
try {
    cargo build --release -p clipboard_share
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build --release -p clipboard_share failed with exit code $LASTEXITCODE"
    }

    cargo build --release -p clipboard_share_ui
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build --release -p clipboard_share_ui failed with exit code $LASTEXITCODE"
    }

    if (-not (Test-Path $releaseExe)) {
        throw "Missing CLI binary: $releaseExe"
    }
    if (-not (Test-Path $uiExe)) {
        throw "Missing UI binary: $uiExe"
    }

    if (Test-Path $distRoot) {
        Remove-Item $distRoot -Recurse -Force
    }

    foreach ($side in @("A", "B")) {
        $sideDir = Join-Path $distRoot "ClipboardShare-$side"
        New-Item -ItemType Directory -Path $sideDir -Force | Out-Null
        Copy-Item $releaseExe (Join-Path $sideDir "clipboard_share.exe")
        Copy-Item $uiExe (Join-Path $sideDir "clipboard_share_ui.exe")
        Copy-Item (Join-Path $projectRoot "portable\$side\config.json") $sideDir
        Copy-Item (Join-Path $projectRoot "portable\start.bat") $sideDir
        Copy-Item (Join-Path $projectRoot "portable\start-ui.bat") $sideDir
        Copy-Item (Join-Path $projectRoot "portable\stop.bat") $sideDir
        Copy-Item (Join-Path $projectRoot "README.md") $sideDir
        Copy-Item (Join-Path $projectRoot "README.zh-CN.md") $sideDir
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
