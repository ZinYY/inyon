// Inyon service worker: caches the app shell and every file of the build, so the installed app
// opens instantly and a second open needs no network for the app itself. API calls, the
// WebSocket and pages under /v/ always go to the network. Paths are under the worker's scope: `/`
// on a hub; a copy hosted under a folder works too.
const CACHE = 'inyon-v3'
const BASE = new URL(self.registration.scope).pathname
const SHELL = [BASE, BASE + 'manifest.webmanifest', BASE + 'icon-192.png', BASE + 'badge-96.png']
// the hashed files of this build; empty in the source, filled in by the build (vite.config.ts)
const ASSETS = []
const assetPaths = () => ASSETS.map((a) => BASE + a)

self.addEventListener('install', (e) => {
  e.waitUntil(
    caches.open(CACHE).then(async (c) => {
      await c.addAll(SHELL)
      // hashed files never change, so only what this build added is fetched
      const have = new Set((await c.keys()).map((r) => new URL(r.url).pathname))
      await c.addAll(assetPaths().filter((p) => !have.has(p)))
    }),
  )
  self.skipWaiting()
})

self.addEventListener('activate', (e) => {
  e.waitUntil(
    (async () => {
      const keys = await caches.keys()
      await Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k)))
      // files of earlier builds, which no page asks for any more
      const keep = new Set(assetPaths())
      const c = await caches.open(CACHE)
      for (const r of await c.keys()) {
        const p = new URL(r.url).pathname
        if (p.startsWith(BASE + 'assets/') && !keep.has(p)) await c.delete(r)
      }
      await self.clients.claim()
    })(),
  )
})

self.addEventListener('fetch', (e) => {
  const url = new URL(e.request.url)
  // /v/ is a page an agent made, served by the hub inside a sandbox: never answered from here, or
  // an offline frame would get the app shell instead
  if (e.request.method !== 'GET' || url.origin !== location.origin || url.pathname.startsWith('/api/') || url.pathname.startsWith('/v/') || url.pathname === '/ws') return
  if (e.request.mode === 'navigate') {
    // network first for pages so updates show up, cached shell when offline
    e.respondWith(fetch(e.request).catch(() => caches.match(BASE)))
    return
  }
  if (url.pathname.startsWith(BASE + 'assets/')) {
    e.respondWith(
      caches.match(e.request).then(
        (hit) => hit || fetch(e.request).then((res) => {
          const copy = res.clone()
          caches.open(CACHE).then((c) => c.put(e.request, copy))
          return res
        }),
      ),
    )
  }
})

// Notifications while Inyon is closed: a hub, or in the web app a paired computer, sends them
// through the browser's push service (apps/web/src/lib/push.ts). The tag matches the open app's,
// so a newer one replaces an older one for the same session.
const APPLE = /iPhone|iPad|Macintosh/.test(navigator.userAgent) && !/Chrome|Chromium|Edg|Firefox/.test(navigator.userAgent)
self.addEventListener('push', (e) => {
  let p = {}
  try {
    p = e.data ? e.data.json() : {}
  } catch {}
  e.waitUntil(
    (async () => {
      const wins = await self.clients.matchAll({ type: 'window', includeUncontrolled: true })
      // the open app shows it itself; Safari ends a subscription whose pushes show nothing, so it always shows
      if (!APPLE && p.kind !== 'test' && wins.some((w) => w.focused && w.visibilityState === 'visible')) return
      await self.registration.showNotification(p.title || 'Inyon', {
        body: p.body || '',
        tag: p.tag || 'inyon',
        renotify: p.kind === 'waiting',
        data: { href: p.href || '/' },
        icon: BASE + 'icon-192.png',
        badge: BASE + 'badge-96.png',
      })
    })(),
  )
})

// A push says which screen to open, as a path in the app. Anyone holding the push key could send
// one, so only a path on this app's own address is opened: "//elsewhere.example" or a full URL to
// another site opens the app's first screen instead.
const appPath = (href) => {
  try {
    const u = new URL(typeof href === 'string' && href.startsWith('/') && !href.startsWith('//') && !href.startsWith('/\\') ? BASE.replace(/\/$/, '') + href : 'x:', self.registration.scope)
    if (u.origin === location.origin && u.pathname.startsWith(BASE)) return u
  } catch {}
  return new URL(BASE, self.registration.scope)
}

self.addEventListener('notificationclick', (e) => {
  e.notification.close()
  const target = appPath(e.notification.data && e.notification.data.href)
  // the open app takes the path within its base, as the push gave it
  const href = '/' + target.pathname.slice(BASE.length) + target.search + target.hash
  e.waitUntil(
    (async () => {
      const wins = await self.clients.matchAll({ type: 'window', includeUncontrolled: true })
      const win = wins.find((w) => new URL(w.url).pathname.startsWith(BASE))
      if (win) {
        await win.focus()
        win.postMessage({ t: 'open', href })
        return
      }
      await self.clients.openWindow(target.href)
    })(),
  )
})
