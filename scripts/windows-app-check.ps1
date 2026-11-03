# Opens the Inyon app's real window on Windows and checks it through WebView2's debug port: the
# page draws itself from the app's state, every page in the side bar opens, a setting saved from
# the window comes back, and Quit in the window ends the app. A script error on the page fails it.
# On a failure it prints window.log and app.log.
#
#   pwsh scripts/windows-app-check.ps1 -Exe path\to\inyon.exe
param(
  [Parameter(Mandatory)] [string] $Exe,
  [int] $Port = 9333
)
$ErrorActionPreference = 'Stop'

# a home of its own, so nothing of the runner's (or a person's) Inyon is touched
$root = Join-Path ([IO.Path]::GetTempPath()) "inyon-app-check-$PID"
New-Item -ItemType Directory -Force $root | Out-Null
$env:USERPROFILE = $root
$env:HOME = $root
$env:INYON_WEBVIEW_DEBUG_PORT = "$Port"
$log = Join-Path $root '.inyon\node\window.log'
$appLog = Join-Path $root '.inyon\node\app.log'

# a copy of the app already running holds its single-instance lock, and the one started here would
# only hand over to it: wait for any to end (an earlier step's), and name them if they don't
$others = { @(Get-CimInstance Win32_Process | Where-Object { $_.Name -like 'inyon*' }) }
$wait = (Get-Date).AddSeconds(60)
while ((& $others).Count -and (Get-Date) -lt $wait) { Start-Sleep -Seconds 2 }
$left = & $others
if ($left.Count) {
  Write-Host "::error::Inyon was already running before the check:"
  $left | ForEach-Object { Write-Host "  $($_.ProcessId) $($_.CommandLine)" }
  exit 1
}

$app = Start-Process -FilePath $Exe -ArgumentList 'app' -PassThru
Write-Host "app started (pid $($app.Id))"

function Fail([string] $why) {
  Write-Host "::error::$why"
  if (Test-Path $log) { Write-Host '--- window.log'; Get-Content $log | Write-Host }
  if (Test-Path $appLog) { Write-Host '--- app.log'; Get-Content $appLog | Write-Host }
  if (!$app.HasExited) { Stop-Process -Id $app.Id -Force }
  exit 1
}

# the window's page, once WebView2 has opened it
$target = $null
$deadline = (Get-Date).AddSeconds(90)
while (!$target -and (Get-Date) -lt $deadline) {
  if ($app.HasExited) { Fail "the app ended at start (exit $($app.ExitCode))" }
  try {
    $target = Invoke-RestMethod "http://127.0.0.1:$Port/json" -TimeoutSec 3 | Where-Object { $_.type -eq 'page' -and $_.url -like 'http://inyon.localhost/*' } | Select-Object -First 1
  } catch {}
  if (!$target) { Start-Sleep -Milliseconds 500 }
}
if (!$target) { Fail "no Inyon page in WebView2 after 90 s" }
Write-Host "page: $($target.url)"

$ws = [Net.WebSockets.ClientWebSocket]::new()
$ws.ConnectAsync([Uri] $target.webSocketDebuggerUrl, [Threading.CancellationToken]::None).Wait()
$script:seq = 0
$script:errors = [Collections.Generic.List[string]]::new()

function Receive {
  $buf = [byte[]]::new(65536)
  $ms = [IO.MemoryStream]::new()
  do {
    $r = $ws.ReceiveAsync([ArraySegment[byte]]::new($buf), [Threading.CancellationToken]::None).GetAwaiter().GetResult()
    $ms.Write($buf, 0, $r.Count)
  } while (!$r.EndOfMessage)
  [Text.Encoding]::UTF8.GetString($ms.ToArray()) | ConvertFrom-Json -Depth 50
}

# one DevTools call; script errors and console errors seen meanwhile are kept
function Cdp([string] $method, $params = @{}) {
  $id = ++$script:seq
  $msg = @{ id = $id; method = $method; params = $params } | ConvertTo-Json -Depth 20 -Compress
  $bytes = [Text.Encoding]::UTF8.GetBytes($msg)
  $ws.SendAsync([ArraySegment[byte]]::new($bytes), 'Text', $true, [Threading.CancellationToken]::None).Wait()
  while ($true) {
    $m = Receive
    if ($m.method -eq 'Runtime.exceptionThrown') { $script:errors.Add($m.params.exceptionDetails.exception.description ?? $m.params.exceptionDetails.text) }
    if ($m.method -eq 'Runtime.consoleAPICalled' -and $m.params.type -eq 'error') { $script:errors.Add(($m.params.args | ForEach-Object { $_.value ?? $_.description }) -join ' ') }
    if ($m.id -eq $id) { return $m }
  }
}

# runs `js` (an async function body) in the page and returns what it returns, or fails. A page that
# loads again while the call runs (the window can navigate once more as it starts) ends the context
# it ran in, with "Execution context was destroyed"; that call is tried again, up to five times.
function Page([string] $what, [string] $js) {
  for ($try = 1; ; $try++) {
    $r = Cdp 'Runtime.evaluate' @{ expression = "(async () => { $js })()"; awaitPromise = $true; returnByValue = $true; timeout = 60000 }
    $said = $r.error.message ?? $r.result.exceptionDetails.exception.description ?? ''
    if ($try -ge 5 -or $said -notmatch 'Execution context was destroyed|Cannot find context') { break }
    Write-Host "$what`: the page loaded again, trying once more"
    Start-Sleep -Seconds 1
  }
  if ($r.error) { Fail "$what`: $($r.error.message)" }
  if ($r.result.exceptionDetails) { Fail "$what`: $($r.result.exceptionDetails.exception.description ?? $r.result.exceptionDetails.text)" }
  $r.result.result.value
}

Cdp 'Runtime.enable' | Out-Null

$helpers = @'
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const drawn = async (what) => {
  for (let i = 0; i < 150; i++) {
    if (document.querySelector('.page > *')) return;
    await sleep(200);
  }
  throw new Error(`${what}: nothing drawn after 30 s`);
};
'@

# drawn from the app's state, so the bridge to the app works both ways
$first = Page 'first page' "$helpers await drawn('the first page'); const s = await call('state'); return JSON.stringify({ version: s.version, host: s.host, nav: document.querySelectorAll('#nav button').length });" | ConvertFrom-Json
Write-Host "drawn: Inyon $($first.version) on $($first.host), $($first.nav) pages in the side bar"
if ($first.nav -lt 7) { Fail "the side bar has $($first.nav) pages, 7 expected" }

# every page in the side bar, clicked like a person would (PowerShell fills in $ names inside @"..."@,
# so the page's code here has no template strings)
$pages = Page 'side bar' @"
$helpers
// a click redraws the side bar, so each button is looked up again
const seen = [];
const count = document.querySelectorAll('#nav button').length;
for (let i = 0; i < count; i++) {
  const label = document.querySelectorAll('#nav button')[i].querySelector('span').textContent;
  document.querySelectorAll('#nav button')[i].click();
  await sleep(400);
  await drawn(label);
  const shown = document.querySelector('#nav button[aria-current="page"] span');
  if (shown?.textContent !== label) throw new Error('clicked ' + label + ', the side bar shows ' + (shown?.textContent ?? 'nothing'));
  seen.push(label);
}
return seen.join(', ');
"@
Write-Host "opened: $pages"

# a setting saved from the window comes back from the app, twice in a row
$theme = Page 'settings' @"
const a = await call('settings', { patch: { theme: 'dark' } });
const b = await call('settings', { patch: { theme: 'light' } });
const s = await call('state');
return [a.theme, b.theme, s.settings.theme].join(' ');
"@
if ($theme -ne 'dark light light') { Fail "theme saved as '$theme', expected 'dark light light'" }
Write-Host "settings: $theme"

if ($script:errors.Count) { Fail "errors on the page:`n$($script:errors -join "`n")" }

# Quit in the window ends the app
# (the page may go before it answers)
try { Page 'quit' "call('quit').catch(() => {}); return 'asked';" | Out-Null } catch {}
if (!$app.WaitForExit(20000)) { Fail "the app was still running 20 s after Quit" }
Write-Host "quit: the app ended (exit $($app.ExitCode))"
if (Test-Path $log) { Write-Host '--- window.log'; Get-Content $log | Write-Host }
