// first: copies settings saved under the old name before anything reads them
import './lib/legacy.ts'
import { lazy, StrictMode, useEffect } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter, Route, Routes } from 'react-router-dom'
import './fonts.css'
import './styles.css'
import { api } from './lib/api.ts'
import { socket } from './lib/socket.ts'
import { useStore } from './lib/store.ts'
import { AppShell } from './components/AppShell.tsx'
import { Toasts } from './components/Toasts.tsx'
import { ConfirmHost } from './components/Confirm.tsx'
import { ContextMenuHost } from './components/ui/ContextMenu.tsx'
import { UpdatePrompt } from './components/UpdatePrompt.tsx'
import { Waiting } from './components/ui/primitives.tsx'
import { Home } from './pages/Home.tsx'
import { Login } from './pages/Login.tsx'
// the overview shows first; the other pages load when opened (and right after start, in the background)
const PanePage = lazy(() => import('./pages/PanePage.tsx').then((m) => ({ default: m.PanePage })))
const Settings = lazy(() => import('./pages/Settings.tsx').then((m) => ({ default: m.Settings })))
const MachinesPage = lazy(() => import('./pages/Machines.tsx').then((m) => ({ default: m.MachinesPage })))
const MachinePage = lazy(() => import('./pages/Machines.tsx').then((m) => ({ default: m.MachinePage })))
const PagesPage = lazy(() => import('./pages/Pages.tsx').then((m) => ({ default: m.PagesPage })))
const TempPage = lazy(() => import('./pages/Temp.tsx').then((m) => ({ default: m.TempPage })))
import { installTapHaptics } from './lib/haptics.ts'
import { installKeepAlive } from './lib/keepalive.ts'
import { installConnWatch } from './lib/connwatch.ts'
import { installHiddenFlag } from './lib/hidden.ts'
import { installNative, isNative, nativeTheme, takePairFragment } from './lib/native.ts'
import { watchInstall } from './lib/install.ts'
import { appHref, BASE, isHosted, publicFile } from './lib/base.ts'
import { flushSnapshots } from './lib/snapshot.ts'
import { Intro, introDue } from './components/Intro.tsx'

function useTheme() {
  const theme = useStore((s) => s.settings.theme)
  useEffect(() => {
    const mq = matchMedia('(prefers-color-scheme: dark)')
    const apply = () => {
      const t = theme === 'system' ? (mq.matches ? 'dark' : 'light') : theme
      document.documentElement.dataset.theme = t
      useStore.setState({ resolvedTheme: t })
      nativeTheme(t)
      document.querySelectorAll('meta[name="theme-color"]').forEach((m) => m.setAttribute('content', t === 'dark' ? '#1b1a18' : '#e9e8e4'))
    }
    apply()
    mq.addEventListener('change', apply)
    return () => mq.removeEventListener('change', apply)
  }, [theme])
}

function App() {
  const authed = useStore((s) => s.authed)
  const setAuthed = useStore((s) => s.setAuthed)
  useTheme()

  useEffect(() => {
    api.session().then((s) => setAuthed(s.authed)).catch(() => setAuthed(false))
    const out = () => setAuthed(false)
    window.addEventListener('inyon:signed-out', out)
    return () => window.removeEventListener('inyon:signed-out', out)
  }, [])

  useEffect(() => {
    if (!authed) return
    socket.start()
    socket.subscribeOverview()
    return () => socket.stop()
  }, [authed])

  // the overview's shape, where the overview lands once the session check answers
  if (authed === null) {
    return (
      <div className="mx-auto w-full max-w-[1280px] px-4 pt-20 lg:px-8 lg:pt-28">
        <Waiting shape="cards" label="Opening Inyon" />
      </div>
    )
  }
  if (!authed)
    return (
      <>
        <Login />
        <ContextMenuHost />
      </>
    )

  return (
    <>
      <Routes>
        <Route element={<AppShell />}>
          <Route index element={<Home />} />
          <Route path="m/:host/p/:pane" element={<PanePage />} />
          <Route path="machines" element={<MachinesPage />} />
          <Route path="machines/:host" element={<MachinePage />} />
          <Route path="pages" element={<PagesPage />} />
          <Route path="pages/:host/:slug" element={<PagesPage />} />
          <Route path="temp" element={<TempPage />} />
          <Route path="temp/:host" element={<TempPage />} />
          <Route path="settings" element={<Settings />} />
          <Route path="*" element={<Home />} />
        </Route>
      </Routes>
      <Toasts />
      <ConfirmHost />
      <ContextMenuHost />
      {/* the phone app's code ships inside the APK, so only a browser can be behind its hub */}
      {!isNative() && <UpdatePrompt />}
    </>
  )
}

installTapHaptics()
installNative()
// a page going away writes what it was about to keep
window.addEventListener('pagehide', flushSnapshots)
// fetch the other pages once the first screen is up, so opening one doesn't wait on the network
setTimeout(() => void Promise.all([import('./pages/PanePage.tsx'), import('./pages/Machines.tsx'), import('./pages/Settings.tsx'), import('./pages/Pages.tsx'), import('./pages/Temp.tsx')]).catch(() => {}), 1500)
installKeepAlive()
installConnWatch()
installHiddenFlag()
// the Android app looks for a newer APK once a day; it never installs one by itself
if (isNative()) setTimeout(() => void import('./lib/appUpdate.ts').then((m) => m.startAppUpdateCheck()), 4000)
// it also keeps a sealed copy of its keys and pairings for the next Inyon app, which installs over it
if (isNative()) setTimeout(() => void import('./lib/upgradeExport.ts').then((m) => m.startUpgradeExport()), 3000)

// before the router reads the address: a pairing code in it is taken out first
takePairFragment()
if (isHosted()) watchInstall()

// the intro once per app session, over the page as it loads; the mark lands on the header's
const intro = introDue()
createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <BrowserRouter basename={appHref('') || undefined}>
      <App />
      {intro && <Intro />}
    </BrowserRouter>
  </StrictMode>,
)

// Offline shell for the installed app, and notifications while it's closed (service workers need
// HTTPS or localhost).
if ('serviceWorker' in navigator && window.isSecureContext && import.meta.env.PROD && !isNative()) {
  navigator.serviceWorker.register(publicFile('sw.js'), { scope: BASE }).then(
    () => void import('./lib/push.ts').then((m) => m.watchPush()),
    () => {},
  )
}
