import { useEffect, useRef, useState } from 'react'
import { Check, CircleCheck, Copy, Link2, Mic, Minus, RefreshCw } from 'lucide-react'
import type { PairedCompanion } from '@inyon/protocol'
import { api, ApiError } from '../lib/api.ts'
import { hubOrigin } from '../lib/hub.ts'
import { toast } from '../lib/store.ts'
import { copyText } from '../lib/clipboard.ts'
import { Button, Field, Pulse, Sheet, Waiting, inputClass } from './ui/primitives.tsx'

type Code = { code: string; expiresAt: number; tlsFingerprint?: string }

/** What a companion may do under contract 1, as the paired summary and the details sheet say it. */
export const COMPANION_CAN = ['Reads the session overview and the chats you open in it', 'Answers prompts and sends text to a session'] as const
export const COMPANION_CANT = "Can't open terminals, files or settings"

/**
 * The code, the hub's address and (when the hub serves HTTPS itself) its certificate's SHA-256 in
 * one string, for an app that takes a pasted link: it can pin the certificate before it sends the
 * code. Contract 1.1: inyon://companion?u=<hub origin>&c=<code>&f=<sha256 hex>.
 */
export const companionLink = (origin: string, code: string, tlsFingerprint?: string) =>
  'inyon://companion?' + new URLSearchParams({ u: origin, c: code, ...(tlsFingerprint ? { f: tlsFingerprint } : {}) })

/** 5:00, 0:42 */
export const mmss = (ms: number) => {
  const s = Math.max(0, Math.ceil(ms / 1000))
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`
}

/**
 * Pair a companion app (Legato and the like): a six-character code it types with the hub's
 * address. The code works once, for five minutes, and a new one is made when it runs out while
 * the sheet is open. The sheet watches the list of companions and says so when the app has
 * claimed it. Like the phone code, it needs a fresh sign-in.
 */
export function CompanionSheet({ open, onClose, onPaired }: { open: boolean; onClose: () => void; onPaired?: (c: PairedCompanion) => void }) {
  const [code, setCode] = useState<Code>()
  const [paired, setPaired] = useState<PairedCompanion>()
  const [needPassword, setNeedPassword] = useState(false)
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string>()
  const [now, setNow] = useState(Date.now)
  // the companions there were when the sheet opened; a new id is the one that just claimed
  const known = useRef<Set<string>>(undefined)
  // the code a renewal is under way or has failed for, so the countdown asks once per code
  const renewing = useRef(0)

  /** Asks the hub for a code; false when it didn't make one (the old code, if any, still works). */
  const fetchCode = async () => {
    setError(undefined)
    setBusy(true)
    try {
      setCode(await api.companionPair())
      setNow(Date.now())
      setNeedPassword(false)
      return true
    } catch (err) {
      if (err instanceof ApiError && err.status === 403) setNeedPassword(true)
      else setError((err as Error).message)
      return false
    } finally {
      setBusy(false)
    }
  }

  useEffect(() => {
    if (!open) return
    setCode(undefined)
    setPaired(undefined)
    setPassword('')
    known.current = undefined
    // the companions there are before the code exists, so the one that claims it is the new one
    void api
      .companions()
      .then(
        (list) => void (known.current = new Set((Array.isArray(list) ? list : []).map((c) => c.id))),
        () => void (known.current = new Set()),
      )
      .then(fetchCode)
  }, [open])

  // the countdown, and a new code once this one has run out
  useEffect(() => {
    if (!open || !code || paired) return
    const t = setInterval(() => {
      const at = Date.now()
      setNow(at)
      if (at < code.expiresAt || renewing.current === code.expiresAt) return
      renewing.current = code.expiresAt
      // a renewal that fails drops the dead code, which shows the error and Try again instead of asking every second
      void fetchCode().then((ok) => ok || setCode(undefined))
    }, 1000)
    return () => clearInterval(t)
  }, [open, code, paired])

  // the app sends the code to the hub, not to this page: look for it in the list
  useEffect(() => {
    if (!open || !code || paired) return
    const t = setInterval(() => {
      void api.companions().then((list) => {
        if (!known.current || !Array.isArray(list)) return
        const fresh = list.find((c) => !known.current!.has(c.id))
        if (!fresh) return
        setPaired(fresh)
        onPaired?.(fresh)
      }, () => {})
    }, 2000)
    return () => clearInterval(t)
  }, [open, code, paired])

  const signIn = async (e: React.FormEvent) => {
    e.preventDefault()
    setBusy(true)
    try {
      await api.login(password)
      setPassword('')
      await fetchCode()
    } catch (err) {
      setError((err as Error).message)
      setBusy(false)
    }
  }

  const renew = async () => {
    if (await fetchCode()) toast({ title: 'New code', body: 'The old one no longer works.', tone: 'info' })
    else toast({ title: "Couldn't make a new code", body: 'The one shown still works.', tone: 'error' })
  }

  const address = hubOrigin()
  const copyAddress = () => copyText(address).then((ok) => ok && toast({ title: 'Hub address copied', tone: 'success' }))
  const copyLink = () => code && copyText(companionLink(address, code.code, code.tlsFingerprint)).then((ok) => ok && toast({ title: 'Pairing link copied', body: 'It works once, like the code.', tone: 'success' }))
  const footer =
    code && !needPassword ? (
      <div className="flex items-center justify-end gap-2">
        {!paired && (
          <Button variant="ghost" onClick={renew} loading={busy}>
            <RefreshCw className="size-3.5" /> New code
          </Button>
        )}
        <Button variant="primary" onClick={onClose}>Done</Button>
      </div>
    ) : undefined

  return (
    <Sheet open={open} onClose={onClose} title="Pair a companion app" icon={<Mic className="size-4" />} footer={footer}>
      {needPassword ? (
        <form onSubmit={signIn} className="space-y-4">
          <p className="text-[13px] leading-relaxed text-fg-2">Pairing an app needs a fresh sign-in. Enter the hub's password once more.</p>
          <Field label="Password">
            <input className={inputClass} type="password" value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="current-password" autoFocus />
          </Field>
          {error && <p className="text-[12.5px] text-danger">{error}</p>}
          <Button variant="primary" type="submit" loading={busy} disabled={!password} className="w-full">
            Continue
          </Button>
        </form>
      ) : !code ? (
        error ? (
          <div className="space-y-3">
            <p className="text-[13px] text-danger">{error}</p>
            <Button variant="primary" onClick={fetchCode} loading={busy}>Try again</Button>
          </div>
        ) : (
          <Waiting label="Making a code" shape="panel" />
        )
      ) : (
        <div className="space-y-4">
          <ol className="space-y-2 text-[13.5px] leading-snug text-fg-2">
            <li className="flex gap-3">
              <span className="flex size-5 shrink-0 items-center justify-center rounded-full bg-sunken font-mono text-[12px] text-fg-3">1</span>
              <span>In Legato, open <span className="font-medium text-fg">Settings › Connections › Inyon</span>.</span>
            </li>
            <li className="flex gap-3">
              <span className="flex size-5 shrink-0 items-center justify-center rounded-full bg-sunken font-mono text-[12px] text-fg-3">2</span>
              <span>Type this code and the hub address.</span>
            </li>
          </ol>
          <div className="flex items-center justify-center gap-1.5" role="img" aria-label={`Pairing code ${code.code.split('').join(' ')}`}>
            {code.code.split('').map((ch, i) => (
              <span key={i} className="contents">
                {i === 3 && <span aria-hidden className="mx-1 h-0.5 w-3 rounded-full bg-line-strong" />}
                <span aria-hidden className="flex h-14 w-11 items-center justify-center rounded-xl border border-line bg-sunken font-mono text-[24px] font-medium text-fg">
                  {ch}
                </span>
              </span>
            ))}
          </div>
          <p className="tabular text-center text-[12.5px] text-fg-3">
            {paired ? 'Used' : `Works once · changes in ${mmss(code.expiresAt - now)}`}
          </p>
          <div className="flex items-center gap-2 rounded-2xl border border-line bg-sunken py-1.5 pl-4 pr-1.5">
            <span className="min-w-0 flex-1 truncate font-mono text-[13px] text-fg">{address}</span>
            <button type="button" onClick={copyAddress} aria-label="Copy the hub address" className="flex size-8 shrink-0 items-center justify-center rounded-full text-fg-3 hover:bg-hover hover:text-fg">
              <Copy className="size-3.5" />
            </button>
          </div>
          {!paired && (
            <button type="button" onClick={copyLink} className="press mx-auto flex items-center gap-1.5 rounded-full px-3 py-1.5 text-[12.5px] text-fg-3 hover:bg-hover hover:text-fg">
              <Link2 className="size-3.5" /> Copy pairing link
            </button>
          )}
          {paired ? (
            <div className="rounded-2xl border border-line bg-elev px-4 py-3" role="status">
              <div className="flex items-center gap-2 text-[14px] font-medium">
                <CircleCheck className="size-4 text-ok" /> {paired.name} is paired
              </div>
              <ul className="mt-2 space-y-1.5 text-[13px] text-fg-2">
                {COMPANION_CAN.map((s) => (
                  <li key={s} className="flex gap-2"><Check className="mt-0.5 size-3.5 shrink-0 text-ok" />{s}</li>
                ))}
                <li className="flex gap-2 text-fg-3"><Minus className="mt-0.5 size-3.5 shrink-0" />{COMPANION_CANT}</li>
              </ul>
            </div>
          ) : (
            <p className="flex items-center gap-2 text-[13px] text-fg-3" role="status">
              <Pulse /> Waiting for the app to send the code…
            </p>
          )}
        </div>
      )}
    </Sheet>
  )
}
