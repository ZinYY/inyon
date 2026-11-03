# Adds this Windows machine to an Inyon hub and runs its connection in the background, as you.
# No admin rights needed.
#
# The hub's "Add machine" sheet generates a one-liner that works from both PowerShell and cmd.exe:
#
#   powershell -NoProfile -ExecutionPolicy Bypass -Command "[Net.ServicePointManager]::SecurityProtocol=3072; & ([scriptblock]::Create((irm '<hub>/node/install.ps1'))) -Hub '<hub>' -Token '<token>'"
#
# The token is base64url ([A-Za-z0-9_-]), so single quotes are always safe around it. It is written
# to %USERPROFILE%\.inyon\node\.env (readable only by you) and never put on a command line that
# Task Scheduler or the process list would show.
param(
  [Parameter(Mandatory = $true)][string]$Hub,
  [Parameter(Mandatory = $true)][string]$Token,
  [switch]$NoPsmux
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue' # Invoke-WebRequest is very slow with the progress bar on PS 5.1
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor 3072

# A shell started over SSH or by another tool can miss the user's PATH (psmux usually lives in
# ~\.local\bin); read it from the registry like a new terminal would.
$env:Path = @([Environment]::GetEnvironmentVariable('Path', 'Machine'), [Environment]::GetEnvironmentVariable('Path', 'User'), $env:Path) -join ';'

$Hub = $Hub.TrimEnd('/')
if ($Token -notmatch '^[A-Za-z0-9_-]{20,}$') { throw 'That token does not look right. Copy the command again from the hub.' }

$Root = Join-Path $env:USERPROFILE '.inyon'
$Bin = Join-Path $Root 'bin'
$Dir = Join-Path $Root 'node'
$OldDir = Join-Path $env:USERPROFILE '.tether-node'
$TaskName = 'Inyon node'
$Startup = [Environment]::GetFolderPath('Startup')

$arch = switch ($env:PROCESSOR_ARCHITECTURE) { 'ARM64' { 'arm64' } default { 'x64' } }

# VPN links drop now and then, and a hub restart cuts a download short: try a few times.
function Get-File([string]$Url, [string]$Out) {
  $tmp = "$Out.download"
  for ($i = 1; $i -le 4; $i++) {
    try {
      Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $tmp -TimeoutSec 900
      Move-Item -Force $tmp $Out
      return
    } catch {
      Remove-Item -Force $tmp -ErrorAction SilentlyContinue
      if ($i -eq 4) { throw "Could not download $Url ($($_.Exception.Message)). Check that this machine can reach the hub, then run the command again." }
      Write-Host "  The download was interrupted; trying again ($i of 3)..."
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

# Stop the running connection first, or Windows keeps its files locked. Before the rename to Inyon
# it was tether-node, started by the 'Tether node' task or a Startup shortcut. A phone link on this
# machine keeps running.
foreach ($t in $TaskName, 'Tether node') { Stop-ScheduledTask -TaskName $t -ErrorAction SilentlyContinue }
Get-CimInstance Win32_Process -Filter "Name='inyon.exe' or Name='inyon-engine.exe' or Name='tether-node.exe'" -ErrorAction SilentlyContinue |
  Where-Object { $_.CommandLine -notmatch '\s"?(link|update|migrate)"?(\s|$)' } |
  ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
Start-Sleep -Milliseconds 500

# tether-node kept its files in .tether-node: they move to .inyon\node, with a link left behind.
# While something still runs from there (a phone link), they stay, and inyon uses them where they are.
$old = Get-Item $OldDir -Force -ErrorAction SilentlyContinue
if ($old -and -not ($old.Attributes -band [IO.FileAttributes]::ReparsePoint) -and -not (Test-Path $Dir)) {
  try {
    New-Item -ItemType Directory -Force -Path $Root | Out-Null
    Move-Item $OldDir $Dir
    New-Item -ItemType Junction -Path $OldDir -Target $Dir | Out-Null
  } catch {
    $Dir = $OldDir
  }
}
New-Item -ItemType Directory -Force -Path $Bin, $Dir | Out-Null

# A phone link may be running the file being replaced: it can't be replaced, but it can be renamed.
function Move-Into([string]$New, [string]$Out) {
  if (Test-Path $Out) {
    try { Remove-Item -Force $Out } catch { Move-Item -Force $Out "$Out.old-$([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds())" }
  }
  Move-Item -Force $New $Out
}

# A program from the hub: it comes gzipped (the engine is about half the size that way) and is
# checked against the hub's checksum before it goes in place.
function Install-Program([string]$Name, [string]$Route, [string]$Want) {
  $out = Join-Path $Bin $Name
  Remove-Item -Force "$out.download", "$out.new" -ErrorAction SilentlyContinue
  for ($try = 1; ; $try++) {
    Get-File "$Hub/node/$Route.gz?os=windows&arch=$arch" "$out.gz"
    Expand-Gzip "$out.gz" "$out.new"
    Remove-Item -Force "$out.gz"
    if ((Get-FileHash -Algorithm SHA256 "$out.new").Hash.ToLower() -eq $Want) {
      Move-Into "$out.new" $out
      return
    }
    Remove-Item -Force "$out.new"
    if ($try -ge 2) { throw "$Name did not arrive intact. Run the command again." }
    Write-Host '  The file did not match its checksum; downloading it again...'
  }
}
function Hub-Sum([string]$Route) { ([string](Invoke-RestMethod -UseBasicParsing "$Hub/node/$Route.sha256?os=windows&arch=$arch")).Trim().ToLower() }

# 1. inyon itself. From 0.19 inyon.exe does the engine's work too, and inyon-engine.exe is a copy
#    of it marked as a windowed program, so no console opens when the task starts it. An inyon.exe
#    with the engine built in carries a mark that says so; whether the hub also lists an engine
#    file doesn't matter.
$exe = Join-Path $Bin 'inyon.exe'
$engine = Join-Path $Bin 'inyon-engine.exe'
Write-Host 'Downloading inyon...'
Install-Program 'inyon.exe' 'inyon' (Hub-Sum 'inyon')
$bytes = [IO.File]::ReadAllBytes($exe)
if ([Text.Encoding]::ASCII.GetString($bytes).Contains('inyon: the built-in engine, windowless copy')) {
  # the subsystem field of the PE header: 2 is a windowed program, 3 a console one
  $at = [BitConverter]::ToInt32($bytes, 0x3c) + 4 + 20 + 68
  $bytes[$at] = 2
  $bytes[$at + 1] = 0
  [IO.File]::WriteAllBytes("$engine.new", $bytes)
  Move-Into "$engine.new" $engine
}

# 2. A small POSIX shell (busybox-w32). The hub drives every machine with short sh scripts, and this
#    keeps them working on Windows without Git Bash or WSL. Served (and pinned) by the hub, and
#    checked against that pin like the two programs above; a copy from before that still matches stays.
function Test-Sum([string]$File, [string]$Want) { (Test-Path $File) -and ((Get-FileHash -Algorithm SHA256 $File).Hash.ToLower() -eq $Want) }
$busybox = Join-Path $Bin 'busybox.exe'
$busyboxSum = Hub-Sum 'busybox.exe'
if (-not (Test-Sum $busybox $busyboxSum)) {
  Write-Host 'Downloading the shell helper (busybox-w32)...'
  Remove-Item -Force "$busybox.new" -ErrorAction SilentlyContinue
  Get-File "$Hub/node/busybox.exe?arch=$arch" "$busybox.new"
  if (-not (Test-Sum "$busybox.new" $busyboxSum)) {
    Remove-Item -Force "$busybox.new"
    throw 'busybox.exe did not arrive intact. Run the command again.'
  }
  # a phone link's shell may be running the old file: it can't be replaced, but it can be renamed
  if (Test-Path $busybox) {
    try { Remove-Item -Force $busybox } catch { Move-Item -Force $busybox "$busybox.old-$([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds())" }
  }
  Move-Item -Force "$busybox.new" $busybox
}

# 3. The secrets file, which inyon reads when it connects. `inyon join` writes it (readable only by
#    you and SYSTEM), as install.sh has it do: it refuses plain http to an address anyone on the
#    network could answer for, and notes the version that joined, so the connection holds this
#    machine to that check rather than letting it in as one added before 0.20. The token goes in
#    the environment, never on a command line.
$env:INYON_TOKEN = $Token
$env:INYON_SH = $busybox
# what it prints on success ("run inyon connect") is this script's job; a refusal goes to stderr
& $exe join $Hub --no-autostart | Out-Null
$joined = $LASTEXITCODE
Remove-Item Env:INYON_TOKEN -ErrorAction SilentlyContinue
if ($joined -ne 0) {
  # the connection stopped above goes back to what it was
  Start-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  throw 'This machine was not added (the reason is above).'
}

# 4. psmux, the Windows tmux. Inyon lists and drives its sessions.
$mux = Get-Command psmux, tmux -ErrorAction SilentlyContinue | Select-Object -First 1
if ($mux -and $mux.Name -eq 'tmux.exe' -and -not (Get-Command psmux -ErrorAction SilentlyContinue)) {
  Write-Host "Found $($mux.Source); Inyon expects psmux's tmux.exe here."
}
if (-not $mux -and -not $NoPsmux) {
  if (Get-Command winget -ErrorAction SilentlyContinue) {
    Write-Host 'Installing psmux with winget...'
    # psmux ships psmux.exe, pmux.exe and tmux.exe (same program), so the hub's `tmux ...` calls work
    winget install psmux --silent --accept-source-agreements --accept-package-agreements | Out-Host
  } else {
    Write-Host 'Note: psmux is not installed. Install it (winget install psmux, or scoop install psmux) so Inyon can see your sessions.'
  }
}

# 5. Start at sign-in and keep running. A scheduled task can restart it if it ever crashes; if task
#    creation is blocked by policy, fall back to the Startup folder. It starts inyon-engine.exe
#    directly: it has no console, so no window shows at sign-in, and the Inyon icon sits in the
#    notification area while it runs.
$runner = if (Test-Path $engine) { $engine } else { $exe }
$started = $false
try {
  $action = New-ScheduledTaskAction -Execute $runner -Argument 'connect' -WorkingDirectory $Dir
  $trigger = New-ScheduledTaskTrigger -AtLogOn -User "$env:USERDOMAIN\$env:USERNAME"
  $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -StartWhenAvailable -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) `
    -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew
  $principal = New-ScheduledTaskPrincipal -UserId "$env:USERDOMAIN\$env:USERNAME" -LogonType Interactive -RunLevel Limited
  Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings `
    -Principal $principal -Description 'Inyon node (outbound connection to your Inyon hub)' -Force | Out-Null
  Start-ScheduledTask -TaskName $TaskName
  $started = $true
  Write-Host "inyon is running (Task Scheduler: '$TaskName'; log: $Dir\node.log)."
} catch {
  Write-Host "Could not create a scheduled task ($($_.Exception.Message)); using the Startup folder instead."
}

if (-not $started) {
  $s = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $Startup "$TaskName.lnk"))
  $s.TargetPath = $runner
  $s.Arguments = 'connect'
  $s.WorkingDirectory = $Dir
  $s.Save()
  Start-Process -FilePath $runner -ArgumentList 'connect' -WorkingDirectory $Dir
  Write-Host "inyon is running and starts when you sign in (log: $Dir\node.log)."
}

# what tether-node set up, now replaced
Unregister-ScheduledTask -TaskName 'Tether node' -Confirm:$false -ErrorAction SilentlyContinue
Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $Startup 'Tether node.lnk')
Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $Dir 'tether-node.exe*')
