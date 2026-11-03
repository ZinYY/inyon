// The runner's side of android-smoke.sh, at 10.0.2.2:4777 from the emulator: "ok" for the
// plain-http check, and under /v/ a page that answers like an agent's page from the hub (the same
// CSP sandbox) and tells the app's frame what native objects it can see.
const PROBE = `<!doctype html><script>
parent.postMessage({
  probe: 1,
  capacitor: typeof window.Capacitor,
  androidBridge: typeof window.androidBridge,
  cookies: typeof window.CapacitorCookiesAndroidInterface,
  http: typeof window.CapacitorHttpAndroidInterface,
  origin: String(self.origin),
}, '*')
</script>`

Bun.serve({
  port: 4777,
  hostname: '0.0.0.0',
  fetch(req) {
    // CORS as the hub answers a phone; without it the app's cross-origin fetch fails either way
    const cors = { 'access-control-allow-origin': '*' }
    if (new URL(req.url).pathname.startsWith('/v/')) {
      return new Response(PROBE, { headers: { ...cors, 'content-type': 'text/html', 'content-security-policy': 'sandbox allow-scripts allow-forms allow-popups allow-downloads' } })
    }
    return new Response('ok', { headers: cors })
  },
})
