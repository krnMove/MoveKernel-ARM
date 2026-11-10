param(
    [string]$OutputPath = "target\fat32.img",
    [uint32]$TotalSectors = 131072
)

$ErrorActionPreference = "Stop"
$bytesPerSector = 512
$reservedSectors = 32
$fatCount = 2
$sectorsPerFat = [uint32][Math]::Ceiling(($TotalSectors - $reservedSectors + 2) / 130.0)
$firstDataSector = $reservedSectors + $fatCount * $sectorsPerFat
$clusterCount = $TotalSectors - $firstDataSector
if ($clusterCount -lt 65525) {
    throw "A FAT32 image needs at least 65525 data clusters. Increase TotalSectors."
}

function Set-Ascii([byte[]]$Buffer, [int]$Offset, [string]$Value) {
    $encoded = [Text.Encoding]::ASCII.GetBytes($Value)
    [Array]::Copy($encoded, 0, $Buffer, $Offset, $encoded.Length)
}

$resolvedPath = [IO.Path]::GetFullPath($OutputPath)
[IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($resolvedPath)) | Out-Null
$stream = [IO.File]::Open(
    $resolvedPath,
    [IO.FileMode]::CreateNew,
    [IO.FileAccess]::ReadWrite,
    [IO.FileShare]::None
)
try {
    $stream.SetLength([int64]$TotalSectors * $bytesPerSector)

    $boot = New-Object byte[] $bytesPerSector
    $boot[0] = 0xeb
    $boot[1] = 0x58
    $boot[2] = 0x90
    Set-Ascii $boot 3 "MOVEKERN"
    [Array]::Copy([BitConverter]::GetBytes([uint16]$bytesPerSector), 0, $boot, 11, 2)
    $boot[13] = 1
    [Array]::Copy([BitConverter]::GetBytes([uint16]$reservedSectors), 0, $boot, 14, 2)
    $boot[16] = $fatCount
    [Array]::Copy([BitConverter]::GetBytes([uint32]$TotalSectors), 0, $boot, 32, 4)
    [Array]::Copy([BitConverter]::GetBytes([uint32]$sectorsPerFat), 0, $boot, 36, 4)
    [Array]::Copy([BitConverter]::GetBytes([uint16]2), 0, $boot, 40, 2)
    [Array]::Copy([BitConverter]::GetBytes([uint16]1), 0, $boot, 42, 2)
    [Array]::Copy([BitConverter]::GetBytes([uint32]2), 0, $boot, 44, 4)
    [Array]::Copy([BitConverter]::GetBytes([uint16]1), 0, $boot, 48, 2)
    [Array]::Copy([BitConverter]::GetBytes([uint16]6), 0, $boot, 50, 2)
    $boot[64] = 0x80
    $boot[66] = 0x29
    [Array]::Copy([BitConverter]::GetBytes([uint32]0x4d4b4152), 0, $boot, 67, 4)
    Set-Ascii $boot 71 "MOVEKERNEL "
    Set-Ascii $boot 82 "FAT32   "
    $boot[510] = 0x55
    $boot[511] = 0xaa

    $stream.Position = 0
    $stream.Write($boot, 0, $boot.Length)
    $stream.Position = 6 * $bytesPerSector
    $stream.Write($boot, 0, $boot.Length)

    $fsInfo = New-Object byte[] $bytesPerSector
    [Array]::Copy([BitConverter]::GetBytes([uint32]0x41615252), 0, $fsInfo, 0, 4)
    [Array]::Copy([BitConverter]::GetBytes([uint32]0x61417272), 0, $fsInfo, 484, 4)
    [Array]::Copy([BitConverter]::GetBytes([uint32]::MaxValue), 0, $fsInfo, 488, 4)
    [Array]::Copy([BitConverter]::GetBytes([uint32]::MaxValue), 0, $fsInfo, 492, 4)
    $fsInfo[510] = 0x55
    $fsInfo[511] = 0xaa
    $stream.Position = $bytesPerSector
    $stream.Write($fsInfo, 0, $fsInfo.Length)
    $stream.Position = 7 * $bytesPerSector
    $stream.Write($fsInfo, 0, $fsInfo.Length)

    foreach ($fatStart in @($reservedSectors, $reservedSectors + $sectorsPerFat)) {
        $fatSector = New-Object byte[] $bytesPerSector
        [Array]::Copy([BitConverter]::GetBytes([uint32]0x0ffffff8), 0, $fatSector, 0, 4)
        [Array]::Copy([BitConverter]::GetBytes([uint32]::MaxValue), 0, $fatSector, 4, 4)
        [Array]::Copy([BitConverter]::GetBytes([uint32]0x0fffffff), 0, $fatSector, 8, 4)
        [Array]::Copy([BitConverter]::GetBytes([uint32]0x0fffffff), 0, $fatSector, 12, 4)
        $stream.Position = [int64]$fatStart * $bytesPerSector
        $stream.Write($fatSector, 0, $fatSector.Length)
    }

    $root = New-Object byte[] $bytesPerSector
    Set-Ascii $root 0 "HELLO   TXT"
    $root[11] = 0x20
    [Array]::Copy([BitConverter]::GetBytes([uint16]3), 0, $root, 26, 2)
    $contents = [Text.Encoding]::ASCII.GetBytes("Hello from FAT32 on QEMU.`r`n")
    [Array]::Copy([BitConverter]::GetBytes([uint32]$contents.Length), 0, $root, 28, 4)
    $stream.Position = [int64]$firstDataSector * $bytesPerSector
    $stream.Write($root, 0, $root.Length)

    $data = New-Object byte[] $bytesPerSector
    [Array]::Copy($contents, $data, $contents.Length)
    $stream.Position = [int64]($firstDataSector + 1) * $bytesPerSector
    $stream.Write($data, 0, $data.Length)
}
finally {
    $stream.Dispose()
}

Write-Output "Created FAT32 image: $resolvedPath ($TotalSectors sectors)"
