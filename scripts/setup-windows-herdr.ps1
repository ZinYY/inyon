# Dependencies for the native Windows/Herdr SSH bridge. Run as the SSH user.
# Existing Herdr servers, agent sessions, SSH settings and hooks stay in place.
$ErrorActionPreference = 'Stop'
Get-Command python -ErrorAction Stop | Out-Null
Get-Command herdr -ErrorAction Stop | Out-Null
$destination = Join-Path $env:USERPROFILE '.inyon\bin'
New-Item -ItemType Directory -Force $destination | Out-Null
$target = Join-Path $destination 'busybox.exe'
$sha = '6e263d154d8548d1eb936f65d1d8312c80df31c45974e48d6335e4dcc0f4f34c'
$name = 'busybox-w64u-FRP-6075-g169694ebd.exe'
if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') {
    $sha = 'e67f873d19d58c535cc9f0c4965ffd622e19b7bab87e3da89cb2185fb54464d7'
    $name = 'busybox-w64a-FRP-6075-g169694ebd.exe'
}
if (!(Test-Path $target)) {
    $temp = "$target.$PID.tmp"
    try {
        Invoke-WebRequest -UseBasicParsing "https://frippery.org/files/busybox/$name" -OutFile $temp
        if ((Get-FileHash -Algorithm SHA256 $temp).Hash.ToLowerInvariant() -ne $sha) {
            throw 'BusyBox did not match the pinned SHA256'
        }
        Move-Item $temp $target
    } finally {
        Remove-Item $temp -Force -ErrorAction SilentlyContinue
    }
}
Write-Output 'Ready. Select Windows / Herdr for this SSH machine in Inyon.'
