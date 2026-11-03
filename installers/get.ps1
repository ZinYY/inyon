# Gets inyon for this PC and sets it up for the Inyon phone app. In PowerShell:
#   irm https://inyon.tech/get.ps1 | iex
# It downloads the inyon command into %USERPROFILE%\.inyon\bin from the website's copy of the
# newest release, checks it against the release's signed checksums, and runs its setup, which
# starts it in the background when you sign in and shows a code to scan with the app. From 0.19
# nothing else is downloaded. No admin rights needed.
$ErrorActionPreference = 'Stop'
# The sha256 of the checksum list this copy was published with. The website fills it in when it
# publishes a release, after checking the list's signature against the release key, so the copy
# it serves needs no other tool to trust that list. Empty here: anywhere else, minisign checks it.
$listSha256 = ''
$ProgressPreference = 'SilentlyContinue' # Invoke-WebRequest is very slow with the progress bar on PS 5.1
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor 3072

$base = if ($env:INYON_DOWNLOAD) { $env:INYON_DOWNLOAD } elseif ($env:TETHER_DOWNLOAD) { $env:TETHER_DOWNLOAD } else { 'https://inyon.tech/download' }
$dir = Join-Path $env:USERPROFILE '.inyon\bin'
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$exe = Join-Path $dir 'inyon.exe'
$tmp = "$exe.download"
$sums = "$exe.sums"

function Sum-Of($file) { (Get-FileHash -Algorithm SHA256 $file).Hash.ToLower() }
function Sum-For($file) { ($lines | Where-Object { $_ -match "^([0-9a-f]{64})\s+\*?$([regex]::Escape($file))$" } | ForEach-Object { $Matches[1] }) | Select-Object -First 1 }
function Refuse($message) {
  Remove-Item -Force $sums, "$sums.minisig" -ErrorAction SilentlyContinue
  throw $message
}
# The version this install asks for: the tag a GitHub address names, the one GitHub calls latest,
# or the one latest.json names on the website or a mirror. Empty for a tag that isn't a version
# (a build like node-latest), which asks for that build and nothing older.
function Asked-Version {
  $b = $base.TrimEnd('/')
  $v = $null
  if ($b -match '/releases/download/([^/]+)$') {
    $v = $Matches[1]
    if ($v -notmatch '^v\d') { return '' }
  } elseif ($b -match '^(.+)/releases/latest/download$') {
    # GitHub answers .../releases/latest with a redirect to the tag
    try {
      $req = [Net.HttpWebRequest]::Create("$($Matches[1])/releases/latest")
      $req.AllowAutoRedirect = $false
      try { $res = $req.GetResponse() } catch [Net.WebException] { $res = $_.Exception.Response }
      if ($res) { $location = [string]$res.Headers['Location']; $res.Close(); if ($location -match '/tag/([^/]+)$') { $v = $Matches[1] } }
    } catch { }
  } else {
    try { $v = [string]((Invoke-WebRequest -UseBasicParsing -Uri "$b/latest.json").Content | ConvertFrom-Json).version } catch { }
  }
  if ($v) { $v = $v -replace '^v', '' }
  if (-not $v -or $v -notmatch '^\d+\.\d+\.\d+[0-9A-Za-z.+-]*$') { Refuse "Couldn't tell which version $base serves, so nothing was installed." }
  $v
}

Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile $sums
$lines = Get-Content $sums
# the list is signed with minisign, with this key
$key = 'RWRTvQv/ifQnD3jarY1oarS0AKdhogH+mdgn/o0NzT/Ro8L43nFqqfp6'
# a mock release for tests signs with a key of its own; it only counts with downloads pointed
# somewhere other than the website (inyon.tech, or its earlier address, which redirects there) or
# GitHub (-match ignores case)
$official = $base -match '^https?://inyon\.(abhashchakraborty\.)?tech(/|$)|^https://github\.com/Abhash-Chakraborty/inyon/releases/'
if (($env:INYON_DOWNLOAD -or $env:TETHER_DOWNLOAD) -and $env:INYON_SIGNING_KEY -and -not $official) { $key = $env:INYON_SIGNING_KEY }
if ($listSha256 -and (Sum-Of $sums) -eq $listSha256) {
  # the list the website checked against the release key when it published this copy
} elseif (Get-Command minisign -ErrorAction SilentlyContinue) {
  # minisign from scoop or the zip from jedisct1/minisign
  try {
    Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS.minisig" -OutFile "$sums.minisig"
    & minisign -V -q -m $sums -x "$sums.minisig" -P $key | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'bad signature' }
  } catch {
    Refuse "The release's checksum list isn't signed by Inyon's release key, so nothing was installed."
  }
  # a good signature isn't enough on its own: an older release's list carries one too. Each
  # release lists its APK as inyon-<version>.apk, so the list has to name the version asked for
  $version = Asked-Version
  if ($version -and -not (Sum-For "inyon-$version.apk")) {
    Refuse "The release's signed checksum list isn't the one for Inyon $version (it doesn't list inyon-$version.apk), so nothing was installed. Try again in a few minutes."
  }
} elseif ($env:INYON_NO_SIGNATURE_CHECK) {
  Write-Host "(the checksum list's signature wasn't checked, as INYON_NO_SIGNATURE_CHECK asked)"
} elseif ($listSha256 -and -not ($env:INYON_DOWNLOAD -or $env:TETHER_DOWNLOAD)) {
  Refuse "The website's checksum list isn't the one this installer was published with: it's probably changing over to a new release. Run the command again in a few minutes."
} else {
  Refuse "Couldn't check the signature of the release's checksum list, because minisign isn't installed, so nothing was installed. Install minisign (scoop install minisign) and run this again, or to trust the list without it, run this first: `$env:INYON_NO_SIGNATURE_CHECK = '1'"
}
Remove-Item -Force $sums, "$sums.minisig" -ErrorAction SilentlyContinue
# Windows on ARM gets its own inyon command from 0.13 on (the old engine behind it stays x64); a
# release without one, and every x64 PC, takes the x64 build
$name = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64' -and (Sum-For 'inyon-windows-arm64.exe')) { 'inyon-windows-arm64.exe' } else { 'inyon-windows-x64.exe' }
function Mismatch {
  Remove-Item -Force $tmp, "$tmp.gz" -ErrorAction SilentlyContinue
  throw "The download didn't match the release's checksum. Run the command again."
}
$want = Sum-For $name
$wantGz = Sum-For "$name.gz"
# the gzipped copy is under half the size; releases before 0.11 have none
if ($wantGz) {
  Write-Host "Downloading $name.gz..."
  Invoke-WebRequest -UseBasicParsing -Uri "$base/$name.gz" -OutFile "$tmp.gz"
  if ((Sum-Of "$tmp.gz") -ne $wantGz) { Mismatch }
  # GZipStream is in System.dll, so this works on PowerShell 5.1 without anything else
  $src = [IO.File]::OpenRead("$tmp.gz")
  try {
    $gz = New-Object IO.Compression.GZipStream($src, [IO.Compression.CompressionMode]::Decompress)
    $dst = [IO.File]::Create($tmp)
    try { $gz.CopyTo($dst) } finally { $dst.Dispose(); $gz.Dispose() }
  } finally { $src.Dispose() }
  Remove-Item -Force "$tmp.gz"
} else {
  Write-Host "Downloading $name..."
  Invoke-WebRequest -UseBasicParsing -Uri "$base/$name" -OutFile $tmp
}
if (-not $want -or (Sum-Of $tmp) -ne $want) { Mismatch }

# a running copy can't be replaced, but it can be renamed; setup restarts the background one
if (Test-Path $exe) {
  try { Remove-Item -Force $exe } catch { Move-Item -Force $exe "$exe.old-$([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds())" }
}
Move-Item -Force $tmp $exe

# new windows find inyon by name: the folder goes on your user Path once (inyon uninstall takes it
# off again), and on this window's. Written to the registry as it is, so entries like %USERPROFILE%
# in it stay as they were. INYON_NO_MODIFY_PATH=1 leaves the Path alone
if (-not $env:INYON_NO_MODIFY_PATH) {
  $envKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
  $userPath = [string]$envKey.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
  $parts = @($userPath -split ';' | Where-Object { $_ })
  $mine = @($dir, '%USERPROFILE%\.inyon\bin')
  if (-not ($parts | Where-Object { $mine -contains $_.TrimEnd('\') })) {
    $envKey.SetValue('Path', ((@($dir) + $parts) -join ';'), [Microsoft.Win32.RegistryValueKind]::ExpandString)
    # tells open programs (Explorer, new terminals) that the environment changed
    [Environment]::SetEnvironmentVariable('INYON_PATH_CHANGED', '1', 'User')
    [Environment]::SetEnvironmentVariable('INYON_PATH_CHANGED', $null, 'User')
    Write-Host "Put inyon on your Path: new windows find it by name."
  }
  $envKey.Close()
  if (-not (@($env:Path -split ';') | Where-Object { $_.TrimEnd('\') -eq $dir })) { $env:Path = "$dir;$env:Path" }
}

$env:INYON_NO_PAUSE = '1'
& $exe setup
