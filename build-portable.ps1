$ErrorActionPreference = "Stop"

$projectRoot = $PSScriptRoot
$releaseExe = Join-Path $projectRoot "target\release\clipboard_share.exe"
$uiExe = Join-Path $projectRoot "target\release\clipboard_share_ui.exe"
# Distribution artifacts only (not cargo target/). Always under packages/.
$distRoot = Join-Path $projectRoot "packages"

function Copy-SharedFiles([string]$destination) {
    Copy-Item (Join-Path $projectRoot "portable\config.listen.example.json") $destination
    Copy-Item (Join-Path $projectRoot "portable\config.connect.example.json") $destination
    Copy-Item (Join-Path $projectRoot "portable\stop.bat") $destination
    Copy-Item (Join-Path $projectRoot "README.md") $destination
    Copy-Item (Join-Path $projectRoot "README.zh-CN.md") $destination
}

function New-Package([string]$stageDir, [string]$zipPath) {
    Compress-Archive `
        -Path (Join-Path $stageDir "*") `
        -DestinationPath $zipPath `
        -CompressionLevel Optimal
}

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

    $cliDir = Join-Path $distRoot "ClipboardShare-CLI"
    New-Item -ItemType Directory -Path $cliDir -Force | Out-Null
    Copy-Item $releaseExe (Join-Path $cliDir "clipboard_share.exe")
    Copy-Item (Join-Path $projectRoot "portable\start.bat") $cliDir
    Copy-SharedFiles $cliDir
    New-Package $cliDir (Join-Path $distRoot "ClipboardShare-CLI.zip")

    $uiDir = Join-Path $distRoot "ClipboardShare-UI"
    New-Item -ItemType Directory -Path $uiDir -Force | Out-Null
    Copy-Item $uiExe (Join-Path $uiDir "clipboard_share_ui.exe")
    Copy-Item $releaseExe (Join-Path $uiDir "clipboard_share.exe")
    Copy-Item (Join-Path $projectRoot "portable\start-ui.bat") $uiDir
    Copy-Item (Join-Path $projectRoot "portable\start.bat") $uiDir
    Copy-SharedFiles $uiDir
    New-Package $uiDir (Join-Path $distRoot "ClipboardShare-UI.zip")

    Write-Host "Portable packages created:"
    Write-Host "  $distRoot\ClipboardShare-CLI.zip"
    Write-Host "  $distRoot\ClipboardShare-UI.zip"
}
finally {
    Pop-Location
}
