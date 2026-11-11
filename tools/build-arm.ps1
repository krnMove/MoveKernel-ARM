param(
    [string]$ImagePath = "target\fat32-test.img",
    [uint32]$TotalSectors = 131072
)

$ErrorActionPreference = "Stop"
$repositoryRoot = Split-Path -Parent $PSScriptRoot

if (-not [IO.Path]::IsPathRooted($ImagePath)) {
    $ImagePath = Join-Path $repositoryRoot $ImagePath
}

$ImagePath = [IO.Path]::GetFullPath($ImagePath)
$imageBuilder = Join-Path $PSScriptRoot "create-fat32-image.ps1"

if (Test-Path -LiteralPath $ImagePath) {
    throw "FAT32 image already exists at '$ImagePath'. Choose another -ImagePath or remove the file manually."
}

Push-Location $repositoryRoot
try {
    Write-Output "Building MoveKernel ARM..."
    & cargo build --target aarch64-unknown-none --release
    if ($LASTEXITCODE -ne 0) {
        throw "ARM kernel build failed with exit code $LASTEXITCODE."
    }

    Write-Output "Creating FAT32 test image..."
    & $imageBuilder -OutputPath $ImagePath -TotalSectors $TotalSectors

    Write-Output "ARM kernel: $(Join-Path $repositoryRoot 'target\aarch64-unknown-none\release\movekernel-arm')"
    Write-Output "FAT32 image: $ImagePath"
}
finally {
    Pop-Location
}
