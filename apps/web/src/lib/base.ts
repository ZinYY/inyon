/**
 * Where the app is served from: `/` on a hub and in the phone app, or a folder for a
 * copy someone hosts under one. Routes inside the
 * app (`/machines`, `/m/…`) never carry it; only addresses the browser sees do.
 */
export const BASE = (import.meta.env?.BASE_URL as string | undefined) || '/'

/** A route as the address bar shows it. */
export const appHref = (route: string, base = BASE) => base.replace(/\/$/, '') + route

/** The route inside the app for an address the browser shows. */
export function routeOf(pathname: string, base = BASE) {
  const root = base.replace(/\/$/, '')
  if (!root || (pathname !== root && !pathname.startsWith(root + '/'))) return pathname
  return pathname.slice(root.length) || '/'
}

/** A file from the app's public folder, like `icon-192.png`. */
export const publicFile = (name: string) => BASE + name.replace(/^\//, '')

/**
 * The web app built for static hosting (GitHub Pages): no hub behind it, so the phone core runs in
 * the page, as in the Android app.
 */
export const isHosted = () => import.meta.env?.VITE_INYON_HOSTED === '1'
