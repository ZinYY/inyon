// The web app used to be installed from this address. This replaces its service worker, clears what
// it cached and unregisters, so the address shows the download page again.
self.addEventListener('install', () => self.skipWaiting())
self.addEventListener('activate', (e) =>
  e.waitUntil(
    (async () => {
      for (const k of await caches.keys()) await caches.delete(k)
      await self.registration.unregister()
      for (const c of await self.clients.matchAll({ type: 'window' })) c.navigate('/')
    })(),
  ),
)
