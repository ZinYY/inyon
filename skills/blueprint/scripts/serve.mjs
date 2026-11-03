// node serve.mjs <site-dir> [--host 0.0.0.0] [--port 8790]
// Serves a Blueprint site. Bind to a private address (a VPN or LAN IP) to open it on your phone.
import { serve } from './lib.mjs';
const a = process.argv.slice(2);
const opt = (k, d) => { const i = a.indexOf(k); return i < 0 ? d : a.splice(i, 2)[1]; };
const host = opt('--host', '127.0.0.1'), port = +opt('--port', 8790);
const s = await serve(a[0] || '.', { host, port });
console.log(`Serving ${a[0] || '.'} at ${s.url}`);
