param(
    [string]$ImagePath = "target\fat32-test.img"
)

$ErrorActionPreference = "Stop"
$repositoryRoot = Split-Path -Parent $PSScriptRoot

if (-not [IO.Path]::IsPathRooted($ImagePath)) {
    $ImagePath = Join-Path $repositoryRoot $ImagePath
}

$ImagePath = [IO.Path]::GetFullPath($ImagePath)
$kernelPath = Join-Path $repositoryRoot "target\aarch64-unknown-none\release\movekernel-arm"

if (-not (Test-Path -LiteralPath $kernelPath -PathType Leaf)) {
    throw "ARM kernel not found. Run tools\build-arm.ps1 first."
}
if (-not (Test-Path -LiteralPath $ImagePath -PathType Leaf)) {
    throw "FAT32 image not found at '$ImagePath'. Run tools\build-arm.ps1 first."
}

$qemu = Get-Command qemu-system-aarch64 -ErrorAction SilentlyContinue
if ($null -eq $qemu) {
    throw "qemu-system-aarch64 was not found. Install QEMU and add it to PATH."
}

$qemuArguments = @(
    "-machine", "virt,gic-version=3",
    "-cpu", "cortex-a57",
    "-m", "128M",
    "-kernel", $kernelPath,
    "-device", "virtio-gpu-device",
    "-device", "virtio-keyboard-device",
    "-drive", "if=none,id=disk0,file=$ImagePath,format=raw",
    "-device", "virtio-blk-device,drive=disk0",
    "-global", "virtio-mmio.force-legacy=false",
    "-display", "sdl",
    "-serial", "none"
)

Write-Output "Starting MoveKernel ARM in QEMU..."
& $qemu.Source @qemuArguments
if ($LASTEXITCODE -ne 0) {
    throw "QEMU exited with code $LASTEXITCODE."
}
