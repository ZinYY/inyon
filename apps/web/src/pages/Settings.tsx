import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import clsx from 'clsx'
import { Bell, BookOpen, Check, ChevronRight, Copy, Download, Heart, KeyRound, LogOut, Mic, Minus, Monitor, Moon, MoreHorizontal, QrCode, ShieldCheck, Smartphone, Sun, Trash2, Vibrate } from 'lucide-react'
import { AGENTS, INSTALL_URL, type CompanionScope, type PairedCompanion } from '@inyon/protocol'
import { api, type HubSettings, type HubUpdate } from '../lib/api.ts'
import { canSleep, toast, useStore } from '../lib/store.ts'
import { haptic } from '../lib/haptics.ts'
import { AgentLogo } from '../lib/skin.tsx'
import { Button, Menu, Pulse, Segmented, Sheet, Toggle, Waiting, inputClass, type MenuItem } from '../components/ui/primitives.tsx'
import { contextMenu } from '../components/ui/ContextMenu.tsx'
import { confirmAction } from '../components/Confirm.tsx'
import { CompanionSheet } from '../components/CompanionSheet.tsx'
import { CustomAgentRows } from '../components/CustomAgents.tsx'
import { CompanionRows } from '../components/CompanionAgent.tsx'
import { HostDot, hostLine, sshAddress } from '../components/ui/status.tsx'
import { askNativeNotifications, isNative, nativeNotificationsOn, nativeNotify, plugin } from '../lib/native.ts'
import { isHosted, publicFile } from '../lib/base.ts'
import { allowBackground, batteryUnrestricted, keepAliveSupported, keepAliveWanted, setKeepAlive } from '../lib/keepalive.ts'
import { copyText } from '../lib/clipboard.ts'
import { appleMobile, standalone } from '../lib/install.ts'
import { enablePush, pushSupported, testPush } from '../lib/push.ts'
import { authorizeCommand, canSsh, isPhone } from '../lib/phone.ts'
import { UploadsCard } from './Machines.tsx'
import { Logo } from '../components/AppShell.tsx'
import { COMMIT, VERSION } from '../lib/version.ts'
import { getRenderer } from '../lib/renderer.ts'
import { PhoneSheet } from '../components/PhoneSheet.tsx'
import { forgetHub, listHubs, onHubs } from '../lib/hubs.ts'
import { checkHub, hubBuild, sameBuild, useHubBuild, watchHub, type Build } from '../lib/updates.ts'
import { checkAppUpdate, type AppUpdate } from '../lib/appUpdate.ts'
import { openLink } from '../lib/format.ts'
import { LastDrop, hubLogId, openConnectHelp } from '../components/ConnectSheet.tsx'
import { isDown, useConnLog, useLastDrop } from '../lib/connlog.ts'

/** The phone app: stay connected while it's in the background, and Android's battery limits. */
function Background() {
  const [on, setOn] = useState(keepAliveWanted)
  const [unrestricted, setUnrestricted] = useState<boolean | undefined>()
  useEffect(() => {
    const check = () => batteryUnrestricted().then(setUnrestricted)
    check()
    // back from the system settings page
    document.addEventListener('visibilitychange', check)
    return () => document.removeEventListener('visibilitychange', check)
  }, [])
  const toggle = (v: boolean) => {
    setOn(v)
    setKeepAlive(v).catch((err) => {
      setOn(!v)
      toast({ title: "Couldn't change that", body: (err as Error).message, tone: 'error' })
    })
  }
  return (
    <Section id="background" title="In the background">
      <Row label="Stay connected" hint="Keeps your machines connected while Inyon is closed, so alerts arrive right away. Shows a small ongoing notification and uses a little more battery.">
        <Toggle on={on} onChange={toggle} label="Stay connected" />
      </Row>
      {unrestricted === false && (
        <Row label="Battery limits" hint="Android is limiting Inyon in the background, so it loses its connection once the screen has been off a while. Allow it to run in the background to keep alerts coming.">
          <button type="button" onClick={() => allowBackground().then((ok) => ok !== undefined && setUnrestricted(ok))} className="h-8 rounded-full border border-line bg-elev px-3.5 shadow-card text-[13px] font-medium hover:bg-hover">
            Allow
          </button>
        </Row>
      )}
    </Section>
  )
}

/** Which Inyon this is, to check for updates or put in a bug report. */
/** The docs on the website: setup, pairing, every screen and command. */
const GUIDE = `${INSTALL_URL}/docs/`
/** Where people can pay for Inyon, if they want to (the repo's FUNDING.yml names the same). */
const SPONSOR = 'https://github.com/sponsors/Abhash-Chakraborty'

function About() {
  const [app, setApp] = useState<{ version: string; build: string } | undefined>()
  useEffect(() => {
    plugin('App')?.getInfo().then((i: { version?: unknown; build?: unknown }) => typeof i?.version === 'string' && setApp({ version: i.version, build: String(i.build ?? '') })).catch(() => {})
  }, [])
  const summary = `Inyon ${VERSION}${COMMIT ? ` (${COMMIT})` : ''}${app ? ` · Android app ${app.version}, build ${app.build}` : ''}`
  return (
    <Section id="about" title="About">
      <Row label="Version" hint={COMMIT ? `Built from commit ${COMMIT}` : undefined}>
        <span className="font-mono text-[13px] text-fg-2">{VERSION}</span>
      </Row>
      {app && (
        <Row label="Android app">
          <span className="font-mono text-[13px] text-fg-2">{app.version}</span>
        </Row>
      )}
      <Row label="Terminal drawing" hint="WebGL draws on the graphics chip; a device without it uses the page.">
        <span className="font-mono text-[13px] text-fg-2">{getRenderer() === 'webgl' ? 'WebGL' : 'Page'}</span>
      </Row>
      <button
        type="button"
        onClick={() => copyText(summary).then((ok) => ok && toast({ title: 'Version copied', tone: 'success' }))}
        className="flex w-full items-center gap-3 px-4 py-3.5 text-left text-[14px] font-medium hover:bg-hover/50"
      >
        <Copy className="size-4 text-fg-3" /> Copy version details
      </button>
      <button
        type="button"
        onClick={() => openLink(GUIDE)}
        className="flex w-full items-center gap-3 px-4 py-3.5 text-left text-[14px] font-medium hover:bg-hover/50"
      >
        <BookOpen className="size-4 text-fg-3" /> How Inyon works <span className="text-fg-3">↗</span>
      </button>
      <button
        type="button"
        onClick={() => openLink(SPONSOR)}
        className="flex w-full items-center gap-3 px-4 py-3.5 text-left text-[14px] font-medium hover:bg-hover/50"
      >
        <Heart className="size-4 text-fg-3" /> Support Inyon <span className="text-fg-3">↗</span>
      </button>
    </Section>
  )
}

/**
 * Updates, always by hand: the hub updates and restarts when you tap Update (or run
 * `inyon update`), and the Android app downloads a new APK that Android then installs.
 */
function Updates() {
  const native = isNative()
  if (isHosted()) {
    return (
      <Section id="updates" title="Updates" desc="Each Inyon release publishes a new version of this web app. Reload to get it.">
        <WebUpdateRow />
      </Section>
    )
  }
  return (
    <Section id="updates" title="Updates" desc={native ? 'Nothing updates by itself. A new version downloads in your browser, and Android asks before installing it.' : 'Nothing updates by itself. Update here, or run inyon update on the hub.'}>
      {native ? <AppUpdateRow /> : <HubUpdateRow />}
    </Section>
  )
}

/** The web app: what's published now, next to what this page runs. */
function WebUpdateRow() {
  const latest = useHubBuild()
  const [checking, setChecking] = useState(false)
  useEffect(() => watchHub(), [])
  const newer = latest && !sameBuild(latest, { version: VERSION, commit: COMMIT || undefined })
  if (newer) {
    return (
      <Row label={`New version: ${latest.version}`} hint={`You have ${VERSION}.`}>
        <Button size="sm" variant="primary" onClick={() => location.reload()}>
          Reload
        </Button>
      </Row>
    )
  }
  return (
    <Row label="Up to date" hint={`This web app is version ${VERSION}.`}>
      <Button size="sm" variant="ghost" loading={checking} disabled={checking} onClick={() => (setChecking(true), void checkHub().finally(() => setChecking(false)))}>
        Check now
      </Button>
    </Row>
  )
}

const pill = (text: string) => <span className="font-mono text-[13px] text-fg-2">{text}</span>

/** The hub: what it runs, what's newer, and the button that updates it. */
function HubUpdateRow() {
  const [info, setInfo] = useState<HubUpdate | undefined>()
  const [failed, setFailed] = useState('')
  const [checking, setChecking] = useState(false)
  // the hub's build when Update was tapped; a different one after a reconnect means it's done
  const [from, setFrom] = useState<Build | undefined>()
  const hub = useHubBuild()
  const load = (refresh = false) => {
    setChecking(refresh)
    return api.hubUpdate(refresh).then(
      (u) => (setInfo(u), setFailed('')),
      (e) => setFailed((e as Error).message),
    ).finally(() => setChecking(false))
  }
  useEffect(() => {
    void load()
    return watchHub()
  }, [])

  const updating = !!from || !!info?.running
  // while it runs: notice a failure from the log, or the end without a restart
  useEffect(() => {
    if (!updating) return
    const t = setInterval(() => {
      api.hubUpdate().then((u) => {
        setInfo(u)
        if (u.error || u.restartNeeded) setFrom(undefined)
      }, () => {})
    }, 4000)
    return () => clearInterval(t)
  }, [updating])
  useEffect(() => {
    if (!from || !hub || sameBuild(hub, from)) return
    // the row turns to "Up to date", and the reload prompt says the rest
    setFrom(undefined)
    haptic('success')
    void load()
  }, [hub, from])

  const start = async () => {
    try {
      // what the hub runs now, commit included, so a restart on new code can be told apart
      if (!hubBuild()) await checkHub()
      const was = hubBuild() ?? { version: info?.current ?? VERSION }
      await api.startHubUpdate()
      setFrom(was)
      setInfo((i) => i && { ...i, error: undefined, running: true })
    } catch (e) {
      toast({ title: "Couldn't start the update", body: (e as Error).message, tone: 'error' })
    }
  }

  if (failed && !info) {
    return (
      <Row label="This hub" hint={`Couldn't check for updates: ${failed}`}>
        <Button size="sm" onClick={() => load(true)}>Try again</Button>
      </Row>
    )
  }
  if (!info) return <Waiting shape="rows" count={1} label="Checking the hub for updates" className="px-4 py-3.5" />
  const available = !!info.latest || !!info.behind
  if (updating) {
    return (
      <Row label="Updating the hub…" hint="It restarts when it's done, and Inyon reconnects by itself. This takes a minute or two.">
        <Pulse className="mx-1" />
      </Row>
    )
  }
  if (info.restartNeeded) return <Row label="Updated" hint="Restart the hub to run the new version. It has no systemd unit Inyon could restart." />
  const running = `This hub runs ${info.current}.`
  const commits = info.behind ? `${info.behind} new ${info.behind === 1 ? 'commit' : 'commits'}` : ''
  return (
    <>
      {info.error && (
        <div className="px-4 py-3 text-[12.5px] leading-snug text-danger">
          The last update didn't finish: {info.error}
          {info.log && <span className="text-fg-3"> The log is in {info.log}.</span>}
        </div>
      )}
      {available ? (
        <Row
          label={info.latest ? `Update available: ${info.latest}` : 'Update available'}
          hint={info.canUpdate ? `${running}${commits ? ` ${commits} to pull.` : ''} It restarts when the update is done.` : `${running} ${info.reason}`}
        >
          {info.canUpdate && (
            <Button size="sm" variant="primary" onClick={start}>
              {info.error ? 'Try again' : 'Update'}
            </Button>
          )}
        </Row>
      ) : (
        <Row label="Up to date" hint={info.reason ? `${running} ${info.reason}` : running}>
          <Button size="sm" variant="ghost" loading={checking} onClick={() => load(true)}>
            Check now
          </Button>
        </Row>
      )}
    </>
  )
}

/** The Android app: its version, the newest APK on the website, and Download. */
function AppUpdateRow() {
  const [u, setU] = useState<AppUpdate | undefined>()
  const [failed, setFailed] = useState(false)
  const [checking, setChecking] = useState(true)
  const check = () => {
    setChecking(true)
    setFailed(false)
    checkAppUpdate(true).then(setU, () => setFailed(true)).finally(() => setChecking(false))
  }
  useEffect(check, [])
  if (u?.latest && u.url) {
    return (
      <Row label={`Update available: ${u.latest}`} hint={`You have ${u.current}. It downloads in your browser; open the file when it's done and Android installs it.`}>
        <Button size="sm" variant="primary" onClick={() => openLink(u.url)}>
          <Download className="size-3.5" /> Download
        </Button>
      </Row>
    )
  }
  return (
    <Row label={failed ? "Couldn't check for updates" : checking && !u ? 'Checking for updates…' : 'Up to date'} hint={u ? `This app is version ${u.current}.` : failed ? 'Check your connection and try again.' : undefined}>
      <Button size="sm" variant="ghost" loading={checking} disabled={checking} onClick={check}>
        {failed ? 'Try again' : 'Check now'}
      </Button>
    </Row>
  )
}

function Section({ id, title, desc, children }: { id?: string; title: string; desc?: string; children: React.ReactNode }) {
  return (
    <section id={id} className="scroll-mt-20">
      <h2 className="text-[15px] font-semibold tracking-tight">{title}</h2>
      {desc && <p className="mt-0.5 text-[13px] text-fg-3">{desc}</p>}
      <div className="mt-3 divide-y divide-line overflow-hidden rounded-3xl border border-line bg-elev shadow-card">{children}</div>
    </section>
  )
}

const SLEEP_CHOICES = [
  { v: '0', label: 'Off' },
  { v: '30', label: '30 min' },
  { v: '60', label: '1 hour' },
  { v: '120', label: '2 hours' },
]

const sleepLabel = (m: number) => (m % 60 === 0 ? `${m / 60} hour${m === 60 ? '' : 's'}` : `${m} min`)

/** From 0.24: this hub puts agents idle this long to sleep (Exit to shell), so they stop using memory. */
function SleepRow() {
  // the whole record, so the row turns up when the hub's record does
  const organize = useStore((s) => s.organize)
  if (!canSleep()) return null
  const rule = organize.sleep?.find((r) => r.scope === 'all')
  // a time set another way (the API takes any minutes) shows as one more choice
  const current = String(rule?.minutes ?? 0)
  const choices = SLEEP_CHOICES.some((c) => c.v === current) ? SLEEP_CHOICES : [...SLEEP_CHOICES, { v: current, label: sleepLabel(Number(current)) }].sort((a, b) => Number(a.v) - Number(b.v))
  const set = (v: string) => void api.setSleep(Number(v) || null).catch((e) => toast({ title: 'Could not change it', body: (e as Error).message, tone: 'error' }))
  return (
    <Row label="Put idle agents to sleep" hint="An agent idle this long exits to its shell, keeping its pane, and Resume conversation brings it back when you need it. Pinned sessions, ones with a message waiting and ones without a conversation to resume stay awake.">
      <Segmented size="sm" value={current} onChange={set} options={choices} />
    </Row>
  )
}

function Row({ label, hint, children }: { label: React.ReactNode; hint?: React.ReactNode; children?: React.ReactNode }) {
  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3.5">
      <div className="min-w-0 flex-[1_1_220px]">
        <div className="text-[14px]">{label}</div>
        {hint && <div className="mt-0.5 text-[12.5px] leading-snug text-fg-3">{hint}</div>}
      </div>
      {children}
    </div>
  )
}

const NAV = [
  ['appearance', 'Appearance'],
  ['agents', 'Agents'],
  ...(isPhone() ? [] : ([['companion', 'Companion']] as const)),
  ['uploads', 'Uploads and clipboard'],
  ['notifications', 'Notifications'],
  ['machines', isPhone() ? 'Computers' : 'Machines'],
  ...(isPhone()
    ? ([['hubs', 'Hubs'], ...(canSsh() ? ([['phone-key', "This phone's key"]] as const) : [])] as const)
    : ([['devices', 'Other devices'], ['paired', 'Paired devices'], ['account', 'Account']] as const)),
  ['updates', 'Updates'],
  ['about', 'About'],
] as const

export function Settings() {
  const settings = useStore((s) => s.settings)
  const set = useStore((s) => s.setSettings)
  const hosts = useStore((s) => s.hosts)
  const setAuthed = useStore((s) => s.setAuthed)
  const [hub, setHub] = useState<HubSettings | null>(null)
  const native = isNative()
  const [perm, setPerm] = useState<string>(native ? 'default' : typeof Notification !== 'undefined' ? Notification.permission : 'unsupported')
  useEffect(() => {
    if (native) void nativeNotificationsOn().then((on) => on !== undefined && setPerm(on ? 'granted' : 'default'))
  }, [native])
  const secure = native || window.isSecureContext
  const allow = () =>
    native
      ? askNativeNotifications().then((ok) => setPerm(ok ? 'granted' : 'denied'))
      : Notification.requestPermission().then((p) => {
          setPerm(p)
          // with permission, the browser can be notified while Inyon is closed too
          if (p === 'granted') void enablePush()
        })
  /** Shows what a notification looks like here, so you know they get through. */
  const testNotification = async () => {
    const title = 'Inyon'
    const body = 'Notifications work. You\u2019ll get one when an agent finishes or needs you.'
    // through the browser's push service, the way they arrive while Inyon is closed
    if (!native && pushSupported() && (await testPush()) > 0) return toast({ title: 'Test sent', body: 'It went through your browser\u2019s push service.', tone: 'success' })
    if (native ? await nativeNotify(title, body, '/settings#notifications', 'test', 'done') : (() => {
      try {
        new Notification(title, { body, tag: 'test', icon: publicFile('icon-192.png'), badge: publicFile('badge-96.png') })
        return true
      } catch {
        return false
      }
    })()) toast({ title: 'Test notification sent', tone: 'success' })
    else toast({ title: "Couldn't show a notification", body: 'Check that notifications are allowed for Inyon in your settings.', tone: 'error' })
  }

  useEffect(() => {
    api.settings().then(setHub).catch(() => {})
    const hash = location.hash.slice(1)
    if (hash) setTimeout(() => document.getElementById(hash)?.scrollIntoView(), 50)
  }, [])

  const saveHub = async (p: Partial<HubSettings['uploads']>, extra: Partial<Omit<HubSettings, 'uploads'>> = {}) => {
    if (!hub) return
    const next = { ...hub, ...extra, uploads: { ...hub.uploads, ...p } }
    setHub(next)
    try {
      setHub(await api.saveSettings(next))
    } catch (e) {
      toast({ title: 'Could not save', body: (e as Error).message, tone: 'error' })
    }
  }

  const agentsByKind = Object.values(AGENTS).map((a) => ({ a, on: hosts.filter((h) => h.agents.includes(a.kind)).map((h) => h.config.name) }))

  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <header className="pt-safe sticky top-0 z-20 border-b border-line bg-bg">
        <div className="flex h-[52px] items-center gap-3 px-4 lg:px-7">
          <span className="md:hidden">
            <Logo />
          </span>
          <h1 className="text-[15px] font-semibold tracking-tight">Settings</h1>
        </div>
      </header>
      <div className="mx-auto flex max-w-[1040px] gap-10 px-4 py-7 lg:px-7">
        <nav className="sticky top-[76px] hidden h-fit w-[190px] shrink-0 space-y-0.5 lg:block">
          {NAV.map(([id, label]) => (
            <a key={id} href={`#${id}`} className="block rounded-full px-2.5 py-1.5 text-[13px] text-fg-2 hover:bg-hover hover:text-fg">
              {label}
            </a>
          ))}
        </nav>
        <div className="min-w-0 flex-1 space-y-9 pb-16">
          <Section id="appearance" title="Appearance">
            <Row label="Theme">
              <Segmented
                value={settings.theme}
                onChange={(theme) => set({ theme })}
                options={[
                  { v: 'light', label: <><Sun className="size-3.5" /> Light</> },
                  { v: 'dark', label: <><Moon className="size-3.5" /> Dark</> },
                  { v: 'system', label: <><Monitor className="size-3.5" /> Auto</> },
                ]}
              />
            </Row>
            <Row label="Match the agent inside sessions" hint="Each agent's colours, logo and reply font. Off keeps the neutral look everywhere.">
              <div className="flex items-center gap-1.5">
                {(['claude', 'codex', 'opencode', 'copilot'] as const).map((k) => (
                  <AgentLogo key={k} kind={k} size={20} />
                ))}
                <span className="ml-2">
                  <Toggle on={settings.agentSkins} onChange={(agentSkins) => set({ agentSkins })} label="Agent skins" />
                </span>
              </div>
            </Row>
            <Row label="Reply font" hint="Auto uses each agent's own: serif for Claude, sans for Codex, mono for OpenCode.">
              <Segmented size="sm" value={settings.replyFont} onChange={(replyFont) => set({ replyFont })} options={[{ v: 'auto', label: 'Auto' }, { v: 'serif', label: 'Serif' }, { v: 'sans', label: 'Sans' }, { v: 'mono', label: 'Mono' }]} />
            </Row>
            <Row label="Tool calls in chat" hint="Everything shows each command, file and search with its timing, and background jobs with their live state. Compact folds long runs into a few lines.">
              <Segmented size="sm" value={settings.toolDetail} onChange={(toolDetail) => set({ toolDetail })} options={[{ v: 'all', label: 'Everything' }, { v: 'compact', label: 'Compact' }]} />
            </Row>
            <Row label={<span className="flex items-center gap-2"><Vibrate className="size-4 text-fg-3" /> Haptic feedback</span>} hint="Taps, sends and answers buzz on phones (Android, and iPhone with iOS 17.4 or later).">
              <Toggle on={settings.haptics} onChange={(haptics) => (set({ haptics }), haptics && setTimeout(() => haptic('success'), 30))} label="Haptics" />
            </Row>
          </Section>

          <Section id="agents" title="Agents" desc="Found on your machines. Inyon picks the right one for each pane automatically. Add your own command to run it as one of these.">
            {agentsByKind.map(({ a, on }) => (
              <Row
                key={a.kind}
                label={
                  <span className="flex items-center gap-3">
                    <AgentLogo kind={a.kind} size={28} />
                    <span>
                      <span className="block font-medium">{a.name}</span>
                      <span className="block text-[12px] text-fg-3">{on.length ? `On ${on.join(', ')}` : 'Not installed on any machine'}</span>
                    </span>
                  </span>
                }
              >
                <div className="flex items-center gap-3 text-[12px] text-fg-3">
                  <code className="rounded-full border border-line bg-code px-2 py-1 font-mono text-[12px] text-fg-2">{a.launch.split(' ')[0]}</code>
                  <span className={clsx('shrink-0 whitespace-nowrap rounded-full px-2 py-0.5 text-[12px]', a.chat ? 'border border-line text-fg' : 'bg-sunken text-fg-3')}>{a.chat ? 'Chat view' : 'Terminal'}</span>
                </div>
              </Row>
            ))}
            {hub && <CustomAgentRows hub={hub} onSaved={setHub} />}
          </Section>

          {!isPhone() && (
            <Section id="companion" title="Companion" desc="An agent you talk to about your other sessions, here, on your phone or on Legato's island. It reads every session, and sends a message or answers a question when you ask it to.">
              <CompanionRows />
            </Section>
          )}

          <section id="sessions" className="scroll-mt-20">
            <h2 className="text-[15px] font-semibold tracking-tight">Live numbers</h2>
            <p className="mt-0.5 text-[13px] text-fg-3">
              Plan limits come from each machine's signed-in Claude account, the same numbers as /usage, so they follow account switches by themselves.
            </p>
            <div className="mt-3 divide-y divide-line overflow-hidden rounded-3xl border border-line bg-elev shadow-card">
              {!hub ? (
                <Waiting shape="rows" count={1} label="Loading hub settings" className="px-4 py-3.5" />
              ) : (
                <Row label="Restart sessions that stop reporting" hint="When a session's settings are swapped under it, its cost and effort stop updating. If on, Inyon resumes it once it's idle, with nothing typed and no background shells.">
                  <Toggle on={!!hub.autoResume} onChange={(v) => saveHub({}, { autoResume: v })} label="Restart sessions that stop reporting" />
                </Row>
              )}
              <SleepRow />
            </div>
          </section>

          <section id="uploads" className="scroll-mt-20">
            <h2 className="text-[15px] font-semibold tracking-tight">Uploads and clipboard</h2>
            <p className="mt-0.5 text-[13px] text-fg-3">Files you attach, paste or drop are copied to ~/.inyon/uploads on the session's machine, and the agent gets their paths.</p>
            <div className="mt-3 divide-y divide-line overflow-hidden rounded-3xl border border-line bg-elev shadow-card">
              {!hub ? (
                <Waiting shape="rows" count={3} label="Loading upload settings" className="px-4 py-3.5" />
              ) : (
                <>
                  <Row label="Delete uploads after" hint="Checked when a machine connects, every few hours, and after an upload. A file cut off half way is deleted after an hour.">
                    <Segmented size="sm" value={String(hub.uploads.retentionDays)} onChange={(v) => saveHub({ retentionDays: Number(v) })} options={[{ v: '1', label: '1 day' }, { v: '7', label: '7 days' }, { v: '30', label: '30 days' }, { v: '0', label: 'Never' }]} />
                  </Row>
                  <Row label="Keep at most" hint="Per machine. Past this, the oldest uploads go first.">
                    <Segmented size="sm" value={String(hub.uploads.maxTotalMB ?? 1024)} onChange={(v) => saveHub({ maxTotalMB: Number(v) })} options={[{ v: '512', label: '500 MB' }, { v: '1024', label: '1 GB' }, { v: '5120', label: '5 GB' }, { v: '0', label: 'No limit' }]} />
                  </Row>
                  <Row label="Largest file">
                    <Segmented size="sm" value={String(hub.uploads.maxMB)} onChange={(v) => saveHub({ maxMB: Number(v) })} options={[{ v: '25', label: '25 MB' }, { v: '100', label: '100 MB' }, { v: '500', label: '500 MB' }]} />
                  </Row>
                </>
              )}
            </div>
            <div className="mt-3 grid gap-3 sm:grid-cols-2">
              {hosts.map((h) => (
                <UploadsCard key={h.config.id} host={h} compact />
              ))}
            </div>
            <Link to="/temp" className="mt-3 flex items-center gap-3 rounded-3xl border border-line bg-elev px-4 py-3.5 shadow-card hover:bg-hover/50">
              <span className="min-w-0 flex-1">
                <span className="block text-[14px]">Agent temp files</span>
                <span className="mt-0.5 block text-[12.5px] leading-snug text-fg-3">Pasted images, screenshots and scratch files agents leave behind.</span>
              </span>
              <ChevronRight className="size-4 shrink-0 text-fg-3" />
            </Link>
          </section>

          <Section id="notifications" title="Notifications">
            <Row label="When an agent finishes" hint="A notice when a session goes from working to idle. Needs-you alerts are always on.">
              <Toggle on={settings.notifyDone} onChange={(notifyDone) => set({ notifyDone })} label="Finished notices" />
            </Row>
            <Row label="When a page moves on" hint="A notice when a task on a page's progress board is done.">
              <Toggle on={settings.notifyPages} onChange={(notifyPages) => set({ notifyPages })} label="Page notices" />
            </Row>
            <Row label={<span className="flex items-center gap-2"><Bell className="size-4 text-fg-3" /> System notifications</span>} hint={!secure ? 'Browsers only allow these over HTTPS or in the installed app. In-app notices still work.' : perm === 'unsupported' && appleMobile() && !standalone() ? 'On iPhone, add Inyon to the Home Screen to get these' : perm === 'granted' ? (pushSupported() ? 'On for this device, also while Inyon is closed' : 'On for this device') : perm === 'denied' ? 'Turned off in your phone\'s settings for Inyon' : 'Alerts when an agent needs you, even with Inyon closed'}>
              {perm !== 'granted' && perm !== 'unsupported' && perm !== 'denied' && secure && (
                <button type="button" onClick={allow} className="h-8 rounded-full border border-line bg-elev px-3.5 shadow-card text-[13px] font-medium hover:bg-hover">
                  Allow
                </button>
              )}
              {perm === 'granted' && (
                <button type="button" onClick={testNotification} className="h-8 rounded-full border border-line bg-elev px-3.5 shadow-card text-[13px] font-medium hover:bg-hover">
                  Send a test
                </button>
              )}
            </Row>
          </Section>

          {keepAliveSupported() && <Background />}

          <Section id="machines" title={isPhone() ? 'Computers' : 'Machines'}>
            {!isPhone() && <ThisConnection />}
            {hosts.map((h) => (
              <Link key={h.config.id} to={`/machines/${encodeURIComponent(h.config.id)}`} className="flex items-center gap-3 px-4 py-3.5 hover:bg-hover/50">
                <HostDot status={h.status} />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[14px] font-medium">{h.config.name}</span>
                  <span className="block truncate text-[12px] text-fg-3">{hostLine(h, `${h.config.kind === 'local' ? 'This hub' : h.config.kind === 'node' ? h.hostname ?? 'Paired machine' : h.config.kind === 'link' ? h.hostname ?? 'Laptop' : sshAddress(h.config, hosts)} · integration ${h.integration === 'installed' ? 'on' : 'off'}`)}</span>
                  <LastDrop id={h.config.id} />
                </span>
                <ChevronRight className="size-4 text-fg-3" />
              </Link>
            ))}
            {hub && (
              <Row label={isPhone() ? 'Computers can message agents' : 'Machines can message agents'} hint="An agent on a joined machine can use inyon send to type into agents on your other machines, each once it's free. Reading sessions and pages needs no switch.">
                <Toggle on={!!hub.machinesSend} onChange={(v) => saveHub({}, { machinesSend: v })} label="Machines can message agents" />
              </Row>
            )}
          </Section>

          {isPhone() ? (
            <>
              <Hubs />
              {canSsh() && <PhoneKey />}
            </>
          ) : (
            <>
          <OtherDevices />
          <PairedDevices />

          <Section id="account" title="Account">
            <ChangePassword />
            <button
              type="button"
              onClick={async () => {
                await api.logout().catch(() => {})
                setAuthed(false)
              }}
              className="flex w-full items-center gap-3 px-4 py-3.5 text-left text-[14px] font-medium text-danger hover:bg-danger-soft/50"
            >
              <LogOut className="size-4" /> Sign out
            </button>
          </Section>
            </>
          )}
          <Updates />
          <About />
          <p className="text-center text-[12px] text-fg-3">Agent marks belong to their owners</p>
        </div>
      </div>
    </div>
  )
}

/** A new password signs out every other browser and phone; this one stays signed in. */
function ChangePassword() {
  const [open, setOpen] = useState(false)
  const [current, setCurrent] = useState('')
  const [next, setNext] = useState('')
  const [again, setAgain] = useState('')
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const reset = () => (setOpen(false), setCurrent(''), setNext(''), setAgain(''), setError(''))
  if (!open) {
    return (
      <button type="button" onClick={() => setOpen(true)} className="flex w-full items-center gap-3 px-4 py-3.5 text-left text-[14px] font-medium hover:bg-hover/50">
        <KeyRound className="size-4 text-fg-3" /> Change password
      </button>
    )
  }
  const save = async (e: React.FormEvent) => {
    e.preventDefault()
    setError('')
    if (next.length < 8) return setError('Use at least 8 characters.')
    if (next !== again) return setError("The two new passwords don't match.")
    setBusy(true)
    try {
      await api.changePassword(current, next)
      haptic('success')
      toast({ title: 'Password changed', body: 'Other devices need to sign in again.', tone: 'success' })
      reset()
    } catch (err) {
      setError((err as Error).message)
    } finally {
      setBusy(false)
    }
  }
  return (
    <form onSubmit={save} className="space-y-2.5 px-4 py-4">
      <input type="password" autoComplete="current-password" value={current} onChange={(e) => setCurrent(e.target.value)} placeholder="Current password" className={`${inputClass} h-11`} autoFocus />
      <input type="password" autoComplete="new-password" value={next} onChange={(e) => setNext(e.target.value)} placeholder="New password (8+ characters)" className={`${inputClass} h-11`} />
      <input type="password" autoComplete="new-password" value={again} onChange={(e) => setAgain(e.target.value)} placeholder="New password again" className={`${inputClass} h-11`} />
      {error && <p className="text-[13px] text-danger">{error}</p>}
      <p className="text-[12.5px] text-fg-3">Every other browser and phone signed in to this hub will need the new password.</p>
      <div className="flex justify-end gap-2 pt-1">
        <Button type="button" variant="ghost" onClick={reset}>Cancel</Button>
        <Button type="submit" variant="primary" loading={busy} disabled={!current || !next || !again}>Change password</Button>
      </div>
    </form>
  )
}

/** The phone app signs in to SSH computers with its own key: one line to add to authorized_keys. */
function PhoneKey() {
  const [key, setKey] = useState('')
  const [error, setError] = useState('')
  useEffect(() => {
    api.phoneKey().then((k) => setKey(k.publicKey), (e) => setError((e as Error).message))
  }, [])
  const copy = (text: string, title: string) => copyText(text).then((ok) => ok && toast({ title, tone: 'success' }))
  return (
    <Section id="phone-key" title="This phone's key" desc="For computers added over SSH instead of with a code: the phone signs in with this key once it's in their ~/.ssh/authorized_keys.">
      <div className="px-4 py-3.5">
        {error ? (
          <p className="text-[13px] text-danger">{error}</p>
        ) : !key ? (
          <Waiting shape="panel" label="Reading this phone's key" />
        ) : (
          <>
            <p className="break-all rounded-xl bg-sunken px-3 py-2.5 font-mono text-[12px] leading-relaxed text-fg-2">{key}</p>
            <div className="mt-3 flex flex-wrap gap-2">
              <Button size="sm" onClick={() => copy(key, 'Key copied')}>
                <Copy className="size-3.5" /> Copy key
              </Button>
              <Button size="sm" variant="ghost" onClick={() => copy(authorizeCommand(key), 'Command copied')}>
                <KeyRound className="size-3.5" /> Copy the command that adds it
              </Button>
            </div>
            <p className="mt-2.5 text-[12.5px] leading-snug text-fg-3">Run that command on each server (in a terminal you already have there). The private half never leaves this phone.</p>
          </>
        )}
      </div>
    </Section>
  )
}

/** Where to open Inyon from a phone or another computer: this same hub address. */
function OtherDevices() {
  const address = window.location.origin
  const local = /^https?:\/\/(localhost|127\.)/.test(address)
  const https = address.startsWith('https:')
  return (
    <Section id="devices" title="Your other devices" desc="Everything runs on this hub. Open the same address anywhere you want Inyon.">
      <Row
        label="Hub address"
        hint={local ? 'This is the hub machine itself. Other devices use its Tailscale or NetBird name or IP with the same port.' : undefined}
      >
        <button
          type="button"
          onClick={() => copyText(address).then((ok) => ok && toast({ title: 'Address copied', tone: 'success' }))}
          className="flex max-w-full items-center gap-2 rounded-full border border-line bg-elev px-3.5 shadow-card py-1.5 font-mono text-[12.5px] hover:bg-hover"
        >
          <span className="truncate">{address}</span>
          <Copy className="size-3.5 shrink-0 text-fg-3" />
        </button>
      </Row>
      <Row
        label={<span className="flex items-center gap-2"><Smartphone className="size-4 text-fg-3" /> In a phone's browser</span>}
        hint={
          <>
            Open the address while the phone is on the same Tailscale or NetBird network.
            {!https && ' Adding it to the home screen as an app, with notifications, needs the hub on HTTPS.'}
          </>
        }
      />
    </Section>
  )
}

const SCOPE_LABEL: Record<CompanionScope, [string, string]> = {
  overview: ['Overview', 'Every session, its state and machine, and live changes'],
  chat: ['Chats', 'The screen, details and alerts of a chat it opens'],
  answer: ['Answer', 'Answer a prompt, once, with its prompt key'],
  send: ['Send', 'Type text into a session; the hub answers once it has landed'],
}

const since = (at: number) => {
  const m = Math.round((Date.now() - at) / 60_000)
  if (m < 1) return 'now'
  if (m < 60) return `${m} min ago`
  if (m < 24 * 60) return `${Math.round(m / 60)} h ago`
  return new Date(at).toLocaleDateString()
}

/**
 * The phones and companion apps paired with this hub, each one revocable, and the codes for the
 * next. A companion's token can't come back, so Remove asks first.
 */
function PairedDevices() {
  const [phones, setPhones] = useState<{ id: string; name: string; pairedAt: number; lastSeen?: number }[]>()
  const [companions, setCompanions] = useState<PairedCompanion[]>()
  const [phoneCode, setPhoneCode] = useState(false)
  const [companionCode, setCompanionCode] = useState(false)
  const [details, setDetails] = useState<PairedCompanion>()
  const loadPhones = () => api.phones().then((p) => setPhones(Array.isArray(p) ? p : []), () => setPhones([]))
  const loadCompanions = () => api.companions().then((c) => setCompanions(Array.isArray(c) ? c : []), () => setCompanions([]))
  useEffect(() => {
    void loadPhones()
    void loadCompanions()
  }, [])
  const forget = async (p: { id: string; name: string }) => {
    try {
      await api.forgetPhone(p.id)
      toast({ title: `${p.name} forgotten`, body: 'It can pair again with a new code.', tone: 'success' })
      void loadPhones()
    } catch (e) {
      toast({ title: "Couldn't forget it", body: (e as Error).message, tone: 'error' })
    }
  }
  const remove = async (c: PairedCompanion) => {
    const yes = await confirmAction({ title: `Remove ${c.name}?`, body: 'Its key stops working now and its connection closes. To use it again, pair it with a new code.', yes: 'Remove', danger: true })
    if (!yes) return
    try {
      await api.forgetCompanion(c.id)
      setDetails(undefined)
      toast({ title: `${c.name} removed`, tone: 'success' })
      void loadCompanions()
    } catch (e) {
      toast({ title: "Couldn't remove it", body: (e as Error).message, tone: 'error' })
    }
  }
  const menu = (c: PairedCompanion): MenuItem[] => [
    { key: 'details', label: 'What it can do', icon: <ShieldCheck className="size-4" />, onSelect: () => setDetails(c) },
    { key: 'remove', label: 'Remove', icon: <Trash2 className="size-4" />, danger: true, separated: true, onSelect: () => void remove(c) },
  ]
  const label = (text: string, n?: number) => (
    <div className="flex items-center gap-2 bg-sunken/60 px-4 pb-1.5 pt-3 text-[12px] font-medium text-fg-3">
      {text}
      {n !== undefined && <span className="tabular rounded-full bg-hover px-1.5 text-[12px]">{n}</span>}
    </div>
  )
  return (
    <Section id="paired" title="Paired devices" desc="Phones and companion apps that reach this hub. Each has its own key, and removing one ends it at once.">
      <Row label={<span className="flex items-center gap-2"><QrCode className="size-4 text-fg-3" /> Inyon app</span>} hint="A phone scans a code, then reaches your machines from any network.">
        <Button size="sm" onClick={() => setPhoneCode(true)}>Show the code</Button>
      </Row>
      <Row label={<span className="flex items-center gap-2"><Mic className="size-4 text-fg-3" /> Companion app</span>} hint="Apps like Legato read your sessions, answer prompts and send text. They type a short code.">
        <Button size="sm" variant="primary" onClick={() => setCompanionCode(true)}>Pair a companion app</Button>
      </Row>
      {label('Phones', phones?.length)}
      {phones && !phones.length && <p className="px-4 py-3 text-[13px] text-fg-3">No phones paired.</p>}
      {phones?.map((p) => (
        <div key={p.id} className="flex items-center gap-3 px-4 py-3 text-[14px]">
          <Smartphone className="size-4 shrink-0 text-fg-3" />
          <span className="min-w-0 flex-1">
            <span className="block truncate font-medium">{p.name}</span>
            <span className="block truncate font-mono text-[12px] text-fg-3">Inyon app · paired {new Date(p.pairedAt).toLocaleDateString()}{p.lastSeen ? ` · seen ${since(p.lastSeen)}` : ''}</span>
          </span>
          <Button size="sm" variant="ghost" onClick={() => forget(p)}>Forget</Button>
        </div>
      ))}
      {label('Companion apps', companions?.length)}
      {companions && !companions.length && <p className="px-4 py-3 text-[13px] text-fg-3">No companion apps. Apps like Legato pair with a short code.</p>}
      {companions?.map((c) => (
        <div key={c.id} className="flex items-start gap-3 px-4 py-3 text-[14px]" {...contextMenu(() => menu(c), { header: c.name })}>
          <Mic className="mt-0.5 size-4 shrink-0 text-fg-3" />
          <span className="min-w-0 flex-1">
            <span className="flex items-center gap-2">
              <span className="truncate font-medium">{c.name}</span>
              <span className="shrink-0 rounded-full border border-line px-1.5 font-mono text-[12px] text-fg-3">contract v{c.contractVersion}</span>
            </span>
            <span className="block font-mono text-[12px] leading-snug text-fg-3">{c.app ? `${c.app} · ` : ''}paired {new Date(c.pairedAt).toLocaleDateString()} · seen {since(c.lastSeen)}</span>
            <span className="mt-1.5 flex flex-wrap gap-1">
              {c.scopes.map((s) => (
                <span key={s} className="rounded-full bg-sunken px-2 py-0.5 text-[12px] text-fg-2">{SCOPE_LABEL[s]?.[0] ?? s}</span>
              ))}
            </span>
          </span>
          <Menu
            align="end"
            width={220}
            header={c.name}
            items={menu(c)}
            trigger={({ toggle, ref }) => (
              <button ref={ref} type="button" onClick={toggle} aria-label={`Options for ${c.name}`} className="-my-1 -mr-1.5 flex size-8 shrink-0 items-center justify-center rounded-full text-fg-3 hover:bg-hover hover:text-fg">
                <MoreHorizontal className="size-4" />
              </button>
            )}
          />
        </div>
      ))}
      <PhoneSheet open={phoneCode} onClose={() => (setPhoneCode(false), void loadPhones())} />
      <CompanionSheet open={companionCode} onClose={() => (setCompanionCode(false), void loadCompanions())} onPaired={() => void loadCompanions()} />
      <Sheet
        open={!!details}
        onClose={() => setDetails(undefined)}
        title={details?.name ?? ''}
        icon={<ShieldCheck className="size-4" />}
        footer={
          <div className="flex items-center justify-between gap-2">
            <Button variant="danger" onClick={() => details && void remove(details)}><Trash2 className="size-3.5" /> Remove</Button>
            <Button variant="primary" onClick={() => setDetails(undefined)}>Done</Button>
          </div>
        }
      >
        {details && (
          <div className="space-y-3">
            <p className="text-[13px] text-fg-3">{details.app ? `${details.app} · ` : ''}contract v{details.contractVersion} · paired {new Date(details.pairedAt).toLocaleDateString()} · seen {since(details.lastSeen)}</p>
            <div className="divide-y divide-line overflow-hidden rounded-2xl border border-line">
              {details.scopes.map((s) => (
                <div key={s} className="flex items-center gap-3 px-4 py-2.5">
                  <span className="min-w-0 flex-1">
                    <span className="block text-[14px]">{SCOPE_LABEL[s]?.[0] ?? s}</span>
                    <span className="block text-[12.5px] text-fg-3">{SCOPE_LABEL[s]?.[1]}</span>
                  </span>
                  <Check className="size-4 shrink-0 text-ok" aria-label="Allowed" />
                </div>
              ))}
              <div className="flex items-center gap-3 px-4 py-2.5">
                <span className="min-w-0 flex-1">
                  <span className="block text-[14px]">Interrupt, terminal, files, settings</span>
                  <span className="block text-[12.5px] text-fg-3">The hub refuses these on every route and WebSocket message.</span>
                </span>
                <Minus className="size-4 shrink-0 text-fg-3" aria-label="Refused" />
              </div>
            </div>
          </div>
        )}
      </Sheet>
    </Section>
  )
}

/** This page's own connection to the hub, once it has dropped at least once. */
function ThisConnection() {
  const drop = useLastDrop('self')
  if (!drop) return null
  return (
    <Row label="This page's connection" hint={<LastDrop id="self" className="text-[12.5px]" />}>
      <Button size="sm" variant="ghost" onClick={() => openConnectHelp('self')}>
        Can't connect?
      </Button>
    </Row>
  )
}

/** The hubs this phone is paired with; forgetting one takes its machines out of the lists. */
function Hubs() {
  const [hubs, setHubs] = useState(listHubs)
  useEffect(() => onHubs(setHubs), [])
  useConnLog()
  return (
    <Section id="hubs" title="Hubs" desc="A hub brings its machines into this app. Pair with one from Add, with the code its web app shows.">
      {!hubs.length && <p className="px-4 py-3.5 text-[13px] text-fg-3">No hubs paired.</p>}
      {hubs.map((h) => (
        <div key={h.id} className="flex items-center gap-3 px-4 py-3 text-[14px]">
          <Logo className="size-[18px]" />
          <span className="min-w-0 flex-1">
            <span className="block truncate font-medium">{h.name}</span>
            <span className="block truncate font-mono text-[12px] text-fg-3">{h.url.replace(/^https?:\/\//, '')}{h.version ? ` · ${h.version}` : ''}</span>
            <LastDrop id={hubLogId(h.id)} />
          </span>
          {isDown(hubLogId(h.id)) && (
            <Button size="sm" variant="ghost" onClick={() => openConnectHelp(hubLogId(h.id))}>
              Can't connect?
            </Button>
          )}
          <Button size="sm" variant="ghost" onClick={() => (forgetHub(h.id), toast({ title: `${h.name} forgotten`, body: 'Its machines are gone from this app. The hub still lists this phone until you forget it there too.', tone: 'success' }))}>
            Forget
          </Button>
        </div>
      ))}
    </Section>
  )
}
