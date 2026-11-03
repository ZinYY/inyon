/**
 * Takes secrets out of text that leaves the device in a report: tokens, tickets, cookies,
 * pairing codes and links, passwords and long keys. Addresses stay, since they're what support
 * needs to see; what an address carries after `?` or `#` doesn't.
 */

const HIDDEN = '[hidden]'

const RULES: [RegExp, string | ((...m: string[]) => string)][] = [
  // pairing links carry the code and the key: inyon://pair?…, inyon://hub?…, tether://pair?…
  [/\b(inyon|tether):\/\/(pair|hub)\?\S*/gi, (_, scheme, kind) => `${scheme}://${kind}?${HIDDEN}`],
  // a URL's query and fragment (a WebSocket ticket, a page token, a #pair? code)
  [/\b((?:https?|wss?):\/\/[^\s?#"'<>]+)[?#][^\s"'<>]*/gi, (_, base) => `${base}?${HIDDEN}`],
  // bearer tokens and cookie headers
  [/\b(bearer)\s+[^\s"',;]+/gi, (_, b) => `${b} ${HIDDEN}`],
  [/\b(cookie|set-cookie|authorization)(["']?\s*[:=]\s*)[^\n]*/gi, (_, k, sep) => `${k}${sep}${HIDDEN}`],
  [/\b((?:__Host-)?(?:inyon|tether)_session)=[^\s;,"']+/gi, (_, k) => `${k}=${HIDDEN}`],
  // key=value and "key": "value" for anything that sounds secret
  // (a quoted value goes whole, spaces and all)
  [/\b((?:token|ticket|password|passwd|secret|code|key|cookie|session|auth|seed|passphrase)[a-z_-]*)(["']?\s*[:=]\s*)(?:"[^"\n]*"?|'[^'\n]*'?|[^\s"',;&}]+)/gi, (_, k, sep) => `${k}${sep}${HIDDEN}`],
  // a hub pairing code as typed or shown: K7Q4-MX2T 493812, K7Q4MX2T:493812
  [/\b[0-9A-Z]{4}-?[0-9A-Z]{4}[: ]\d{6}\b/g, HIDDEN],
  // a private key block
  [/-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?(-----END [A-Z ]*PRIVATE KEY-----|$)/g, HIDDEN],
  // anything long and random enough to be a token or a key (a computer's id included)
  // (letters and digits mixed, so a long word or a path isn't one)
  [/(?<![A-Za-z0-9_\-+/])(?=[A-Za-z0-9_\-+/]*\d)(?=[A-Za-z0-9_\-+/]*[A-Za-z])[A-Za-z0-9_\-+/]{24,}={0,2}/g, HIDDEN],
]

export function redact(text: string): string {
  let out = text
  for (const [re, to] of RULES) out = out.replace(re, to as never)
  return out
}
