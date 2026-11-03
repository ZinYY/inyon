// Evaluates one expression in the app's WebView over the DevTools protocol and prints the value.
// The call counts as a user gesture, so it may open file choosers.
//   bun webview-eval.ts <webSocketDebuggerUrl> <expression>
const [url, expression] = Bun.argv.slice(2)
if (!url || !expression) {
  console.error('usage: webview-eval.ts <ws url> <expression>')
  process.exit(2)
}

const ws = new WebSocket(url)
const timer = setTimeout(() => {
  console.error('no answer from the WebView')
  process.exit(1)
}, 30_000)

ws.onopen = () => ws.send(JSON.stringify({ id: 1, method: 'Runtime.evaluate', params: { expression, userGesture: true, awaitPromise: true, returnByValue: true } }))
ws.onerror = () => {
  console.error('could not reach the WebView')
  process.exit(1)
}
ws.onmessage = (m) => {
  const msg = JSON.parse(String(m.data))
  if (msg.id !== 1) return
  clearTimeout(timer)
  const r = msg.result
  if (r?.exceptionDetails) {
    console.error(JSON.stringify(r.exceptionDetails))
    process.exit(1)
  }
  console.log(JSON.stringify(r?.result?.value ?? null))
  process.exit(0)
}
