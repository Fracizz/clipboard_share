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

    # Dedicated tray-UI package (same binaries; A config as default template).
    $uiDir = Join-Path $distRoot "ClipboardShare-UI"
    New-Item -ItemType Directory -Path $uiDir -Force | Out-Null
    Copy-Item $uiExe (Join-Path $uiDir "clipboard_share_ui.exe")
    Copy-Item $releaseExe (Join-Path $uiDir "clipboard_share.exe")
    Copy-Item (Join-Path $projectRoot "portable\A\config.json") (Join-Path $uiDir "config.json")
    Copy-Item (Join-Path $projectRoot "portable\B\config.json") (Join-Path $uiDir "config.B.example.json")
    Copy-Item (Join-Path $projectRoot "portable\start-ui.bat") $uiDir
    Copy-Item (Join-Path $projectRoot "portable\start.bat") $uiDir
    Copy-Item (Join-Path $projectRoot "portable\stop.bat") $uiDir
    Copy-Item (Join-Path $projectRoot "README.md") $uiDir
    Copy-Item (Join-Path $projectRoot "README.zh-CN.md") $uiDir
    Compress-Archive `
        -Path (Join-Path $uiDir "*") `
        -DestinationPath (Join-Path $distRoot "ClipboardShare-UI.zip") `
        -CompressionLevel Optimal

    Write-Host "Portable packages created:"
    Write-Host "  $distRoot\ClipboardShare-A.zip"
    Write-Host "  $distRoot\ClipboardShare-B.zip"
    Write-Host "  $distRoot\ClipboardShare-UI.zip"
}
finally {
    Pop-Location
}
