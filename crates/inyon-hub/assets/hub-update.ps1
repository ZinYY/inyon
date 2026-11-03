# Updates a tether-node that install.ps1 set up, from the hub it's paired with. The hub fills in the
# checksum, version and architecture below, writes this to %USERPROFILE%\.tether-node\update.ps1
# and runs it with -Launch, which starts it again as a one-off scheduled task: stopping the
# "Tether node" task must not end the update with it. It works with any tether-node, so a version
# from before `tether-node update` can be updated too. Nothing is replaced until the download
# matches the checksum, and the old copy comes back if the new one doesn't start.
param([switch]$Launch)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue' # Invoke-WebRequest is very slow with the progress bar on PS 5.1
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor 3072

$Want = '@@SHA256@@'
$Version = '@@VERSION@@'
$Arch = '@@ARCH@@'
$Dir = Join-Path $env:USERPROFILE '.tether-node'
$Exe = Join-Path $Dir 'tether-node.exe'
$TaskName = 'Tether node'
$UpdateTask = 'Tether update'

function Say([string]$Line) { Set-Content -Path (Join-Path $Dir 'update-status') -Value $Line -Encoding ASCII }
function Log([string]$Line) { Add-Content -Path (Join-Path $Dir 'update.log') -Value "$(Get-Date -Format s) $Line" -Encoding ASCII }

if ($Launch) {
  $arg = '-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File "' + $PSCommandPath + '"'
  try {
    $a = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument $arg
    $p = New-ScheduledTaskPrincipal -UserId "$env:USERDOMAIN\$env:USERNAME" -LogonType Interactive -RunLevel Limited
    $s = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Minutes 30)
    Register-ScheduledTask -TaskName $UpdateTask -Action $a -Principal $p -Settings $s -Force | Out-Null
    Start-ScheduledTask -TaskName $UpdateTask
  } catch {
    # task creation blocked by policy: a hidden window of its own instead
    Start-Process -FilePath 'powershell.exe' -ArgumentList $arg -WindowStyle Hidden
  }
  exit 0
}

# install.ps1 keeps the hub address in .env next to tether-node.exe
function Get-Hub {
  foreach ($line in (Get-Content (Join-Path $Dir '.env') -ErrorAction SilentlyContinue)) {
    if ($line -match '^TETHER_HUB=(.+)$') { return $Matches[1].Trim().TrimEnd('/') }
  }
  throw "There's no hub address in $Dir\.env"
}

# VPN links drop now and then: try a download a few times
function Get-File([string]$Url, [string]$Out) {
  for ($i = 1; $i -le 3; $i++) {
    try {
      Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $Out -TimeoutSec 900
      return
    } catch {
      Remove-Item -Force $Out -ErrorAction SilentlyContinue
      if ($i -eq 3) { throw "Couldn't download $Url ($($_.Exception.Message))" }
      Start-Sleep -Seconds (3 * $i)
    }
  }
}

function Expand-Gzip([string]$In, [string]$Out) {
  $src = [IO.File]::OpenRead($In)
  $gz = New-Object IO.Compression.GZipStream($src, [IO.Compression.CompressionMode]::Decompress)
  $dst = [IO.File]::Create($Out)
  try { $gz.CopyTo($dst) } finally { $dst.Dispose(); $gz.Dispose(); $src.Dispose() }
}

# antivirus can hold a file it's still scanning for a moment
function Move-Retry([string]$From, [string]$To) {
  for ($i = 1; ; $i++) {
    try { Move-Item -Force $From $To; return } catch { if ($i -ge 5) { throw }; Start-Sleep -Seconds 1 }
  }
}

# the running copy, under its own name or renamed aside
function Get-Node { Get-Process -Name 'tether-node' -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $Exe -or $_.Path -like "$Exe.old-*" } }

function Stop-Node {
  if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) { Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue }
  Get-Node | Stop-Process -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 1
}

# the task install.ps1 made, or, where policy blocked tasks, the way its Startup shortcut starts it
function Start-Node {
  if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) { Start-ScheduledTask -TaskName $TaskName }
  else { Start-Process -FilePath $Exe -WorkingDirectory $Dir -WindowStyle Hidden }
}

# The new copy writes node.json with its pid and version as it starts; it must still be running a
# few seconds later.
function Test-Started {
  $file = Join-Path $Dir 'node.json'
  for ($i = 0; $i -lt 45; $i++) {
    $n = $null
    try { $n = Get-Content $file -Raw -ErrorAction Stop | ConvertFrom-Json } catch {}
    if ($n -and $n.version -eq $Version) {
      Start-Sleep -Seconds 5
      return [bool](Get-Process -Id $n.pid -ErrorAction SilentlyContinue)
    }
    Start-Sleep -Seconds 1
  }
  return $false
}

$new = "$Exe.new"
$gz = "$Exe.gz"
$old = $null
try {
  Log "updating tether-node to $Version"
  $hub = Get-Hub
  Remove-Item -Force $new, $gz -ErrorAction SilentlyContinue
  Get-File "$hub/node/tether-node.exe.gz?arch=$Arch" $gz
  Expand-Gzip $gz $new
  Remove-Item -Force $gz
  if ((Get-FileHash -Algorithm SHA256 $new).Hash.ToLower() -ne $Want) {
    Remove-Item -Force $new
    throw "The download didn't match the hub's checksum, so nothing was changed"
  }

  # a running .exe can't be overwritten, but it can be renamed
  $old = "$Exe.old-$([DateTimeOffset]::UtcNow.ToUnixTimeSeconds())"
  Move-Retry $Exe $old
  try { Move-Retry $new $Exe } catch { Move-Item -Force $old $Exe; $old = $null; throw }
  Remove-Item -Force (Join-Path $Dir 'node.json') -ErrorAction SilentlyContinue
  Stop-Node
  Start-Node
  if (-not (Test-Started)) { throw "The new version didn't start, so the previous one is running again" }
  Remove-Item -Force $old -ErrorAction SilentlyContinue
  $old = $null
  Log 'done'
  Say "done $Version"
} catch {
  $msg = $_.Exception.Message
  # replaced but not running: put the old copy back
  if ($old -and (Test-Path $old)) {
    try {
      Stop-Node
      # the new copy may still be held: rename it aside rather than overwrite it
      $bad = "$Exe.old-$([DateTimeOffset]::UtcNow.ToUnixTimeSeconds() + 1)"
      if (Test-Path $Exe) { Move-Retry $Exe $bad }
      Move-Retry $old $Exe
      # older versions don't clear .old- copies when they start
      Remove-Item -Force $bad -ErrorAction SilentlyContinue
    } catch { Log "couldn't put the previous version back: $($_.Exception.Message)" }
  }
  Log "failed: $msg"
  Say "failed: $msg"
  # whatever went wrong, leave tether-node running
  if (-not (Get-Node)) { try { Start-Node } catch { Log "couldn't start tether-node: $($_.Exception.Message)" } }
} finally {
  Remove-Item -Force $new, $gz -ErrorAction SilentlyContinue
  Unregister-ScheduledTask -TaskName $UpdateTask -Confirm:$false -ErrorAction SilentlyContinue
}
