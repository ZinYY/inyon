import { useEffect, useMemo, useRef, useState } from 'react'
import clsx from 'clsx'
import { Copy, ShieldAlert, ShieldCheck } from 'lucide-react'
import type { HostConfig } from '@inyon/protocol'
import { api, type ServerForm } from '../lib/api.ts'
import { toast, useStore } from '../lib/store.ts'
import { copyText } from '../lib/clipboard.ts'
import { authorizeCommand, canSsh } from '../lib/phone.ts'
import { useOpened } from '../lib/useOpened.ts'
import { Button, Field, Segmented, Sheet, inputClass } from './ui/primitives.tsx'
import { Select } from './ui/Select.tsx'

type Auth = NonNullable<ServerForm['auth']>

/**
 * Adding or editing a server in the phone app, for machines Inyon can't run on. Either the phone
 * connects to it itself over SSH (you give the address and how to sign in, then confirm the
 * server's fingerprint the first time), or a paired computer that can already reach it does, with
 * its own keys and ~/.ssh/config. The web app can only do the second: a browser has no SSH.
 */
export function ServerSheet({ open, onClose, edit, onLaptop }: { open: boolean; onClose: () => void; edit?: HostConfig; onLaptop?: () => void }) {
  const [name, setName] = useState('')
  const [target, setTarget] = useState('')
  const [port, setPort] = useState('')
  const [auth, setAuth] = useState<Auth>('phone-key')
  // '' is the phone itself, otherwise the paired computer that connects
  const [from, setFrom] = useState('')
  const hosts = useStore((s) => s.hosts)
  const computers = useMemo(() => hosts.filter((h) => h.config.kind === 'link'), [hosts])
  const computer = computers.find((h) => h.config.id === from)
  const direct = canSsh()
  // in a browser with no paired computer there's nothing that could connect
  const blocked = !direct && !computer
  const [password, setPassword] = useState('')
  const [privateKey, setPrivateKey] = useState('')
  const [passphrase, setPassphrase] = useState('')
  const [phoneKey, setPhoneKey] = useState('')
  const [keyError, setKeyError] = useState('')
  // the fingerprint the server showed, waiting for a yes; and the address it belongs to
  const [found, setFound] = useState<{ fingerprint: string; keyType: string; at: string } | null>(null)
  const [busy, setBusy] = useState(false)
  const fingerprint = useRef<HTMLDivElement>(null)

  useOpened(open, edit?.id, () => {
    setName(edit?.name ?? '')
    setTarget(edit?.target ?? '')
    setPort(edit?.port ? String(edit.port) : '')
    setAuth(edit?.auth ?? 'phone-key')
    setFrom(edit?.via ?? (direct ? '' : computers[0]?.config.id ?? ''))
    setPassword('')
    setPrivateKey('')
    setPassphrase('')
    setFound(null)
    setKeyError('')
    if (direct) api.phoneKey().then((k) => setPhoneKey(k.publicKey), (err) => setKeyError((err as Error).message || 'unknown error'))
  })

  // the fingerprint appears under the fold of a phone-sized sheet: bring it into view to be read
  useEffect(() => {
    if (found) fingerprint.current?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
  }, [found?.fingerprint, found?.at])

  const address = `${target.trim()}:${port || 22}`
  const noUser = !!target.trim() && !target.includes('@')
  const known = edit?.hostKey && `${edit.target}:${edit.port ?? 22}` === address ? edit.hostKey : undefined
  const confirmed = found?.at === address ? found.fingerprint : known
  const keptSecret = !!edit && !edit.via && edit.auth === auth
  const signedIn = auth === 'phone-key' || auth === 'none' || keptSecret || (auth === 'password' ? !!password : !!privateKey.trim())
  const ready = !!name.trim() && (computer ? /^[^\s-]\S*$/.test(target.trim()) : direct && /^[^@\s]+@[^@\s]+$/.test(target.trim()) && signedIn)

  /** Asks the server for its key, so you can compare it before the phone ever signs in. */
  const check = async () => {
    setBusy(true)
    try {
      const r = await api.probe(target.trim(), port ? Number(port) : undefined)
      setFound({ ...r, at: address })
    } catch (err) {
      toast({ title: "Couldn't reach the server", body: (err as Error).message, tone: 'error' })
    } finally {
      setBusy(false)
    }
  }

  const save = async (hostKey?: string) => {
    setBusy(true)
    try {
      const body: ServerForm = computer
        ? { name: name.trim(), target: target.trim(), port: port ? Number(port) : undefined, via: computer.config.id }
        : {
            name: name.trim(),
            target: target.trim(),
            port: port ? Number(port) : undefined,
            via: '',
            hostKey,
            auth,
            password: auth === 'password' && password ? password : undefined,
            privateKey: auth === 'key' && privateKey.trim() ? privateKey.trim() : undefined,
            passphrase: auth === 'key' && passphrase ? passphrase : undefined,
          }
      if (edit) await api.editServer(edit.id, body)
      else await api.addServer(body)
      toast({ title: edit ? 'Server updated' : 'Server added', body: computer ? `Connecting from ${computer.config.name}…` : 'Connecting over SSH…', tone: 'success' })
      onClose()
    } catch (err) {
      toast({ title: 'Could not save', body: (err as Error).message, tone: 'error' })
    } finally {
      setBusy(false)
    }
  }

  const submit = (e: React.FormEvent) => {
    e.preventDefault()
    if (!ready || busy) return
    // the computer's own ssh checks the server's key against its known_hosts
    if (computer) return save()
    if (found && found.at === address) return save(found.fingerprint)
    if (confirmed) return save(confirmed)
    check()
  }

  const changed = !computer && !!known && found?.at === address && found.fingerprint !== known
  const asking = !computer && !!found && found.at === address && found.fingerprint !== known

  return (
    <Sheet
      open={open}
      onClose={onClose}
      title={edit ? `Edit ${edit.name}` : 'Connect over SSH'}
      footer={
        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose}>Cancel</Button>
          <Button variant="primary" type="submit" form="server-form" loading={busy} disabled={!ready}>
            {asking ? (edit ? 'Trust and save' : 'Trust and add') : confirmed || computer ? (edit ? 'Save' : 'Add server') : 'Continue'}
          </Button>
        </div>
      }
    >
      <form id="server-form" onSubmit={submit} className="space-y-4">
        {!edit && (
          <p className="text-[13px] leading-relaxed text-fg-2">
            For servers you can't install Inyon on. Nothing is installed there: {!direct ? 'a computer you paired, which can already reach the server, signs in over SSH. A browser can\u2019t open SSH connections itself.' : computers.length ? 'this phone, or a computer you paired that can already reach it, signs in over SSH.' : 'this phone signs in over SSH, so it has to be able to reach it (same Wi-Fi, or a VPN like Tailscale or NetBird).'}
          </p>
        )}
        {!direct && !computers.length && (
          <p className="rounded-2xl border border-line bg-code p-3 text-[12.5px] leading-relaxed text-fg-2">
            Pair a computer first, one that can reach this server. Or use the Android app, which can sign in to servers itself.
          </p>
        )}
        {(computers.length > 1 || (direct && computers.length > 0)) && (
          <Field label="Connect from">
            <Select
              title="Connect from"
              aria-label="Connect from"
              value={from}
              onChange={setFrom}
              options={[
                ...(direct ? [{ value: '', label: 'This phone', hint: 'The phone signs in itself. It needs a route to the server.' }] : []),
                ...computers.map((h) => ({ value: h.config.id, label: h.config.name, hint: h.status === 'online' ? 'Uses its own SSH keys and config' : 'Offline right now' })),
              ]}
            />
          </Field>
        )}
        {!blocked && (
          <>
          <Field label="Name">
            <input className={inputClass} value={name} onChange={(e) => setName(e.target.value)} placeholder="Work server" autoFocus={!edit} />
          </Field>
          <div className="grid grid-cols-[1fr_92px] gap-3">
            <Field label={computer ? 'Address or SSH alias' : 'User and address'} hint={!computer && noUser ? 'Add the user name too, like you@server' : undefined}>
              <input
                className={clsx(inputClass, 'font-mono text-[13.5px]')}
                value={target}
                onChange={(e) => setTarget(e.target.value.trim())}
                placeholder={computer ? 'work-db or you@10.0.0.5' : 'you@100.64.0.5'}
                inputMode="email"
                autoCapitalize="none"
                autoCorrect="off"
                spellCheck={false}
              />
            </Field>
            <Field label="Port">
              <input className={inputClass} value={port} onChange={(e) => setPort(e.target.value.replace(/\D/g, ''))} placeholder="22" inputMode="numeric" />
            </Field>
          </div>

          {computer ? (
            <p className="rounded-2xl border border-line bg-code p-3 text-[12.5px] leading-relaxed text-fg-2">
              {computer.config.name} runs <code className="font-mono">ssh</code> for every command, with its own keys, <code className="font-mono">~/.ssh/config</code> and known hosts. It has to get in without a password prompt: check that <code className="whitespace-nowrap font-mono">ssh {target.trim() || 'work-db'}</code> works there first.
            </p>
          ) : (
            <>
              <Field label="Sign in with">
                <Segmented full value={auth} onChange={setAuth} options={[{ v: 'phone-key', label: 'Phone key' }, { v: 'password', label: 'Password' }, { v: 'key', label: 'Key' }, { v: 'none', label: 'None' }]} />
              </Field>
              {auth === 'none' && (
                <p className="text-[12.5px] leading-relaxed text-fg-3">For servers that let you in without a key or password, like Tailscale SSH or an account with no password.</p>
              )}
              {auth === 'phone-key' && (
                <div className="rounded-2xl border border-line bg-code p-3">
                  <p className="text-[12.5px] leading-relaxed text-fg-2">Run this once on the server to let this phone in. The private half of the key never leaves the phone.</p>
                  {phoneKey ? (
                    <Button size="sm" className="mt-2.5" onClick={() => copyText(authorizeCommand(phoneKey)).then((ok) => ok && toast({ title: 'Command copied', tone: 'success' }))}>
                      <Copy className="size-3.5" /> Copy the command
                    </Button>
                  ) : keyError ? (
                    <p className="mt-2 text-[12.5px] text-danger">Couldn't read this phone's key ({keyError}). Sign in with a password or a private key instead.</p>
                  ) : null}
                </div>
              )}
              {auth === 'password' && (
                <input
                  type="password"
                  className={inputClass}
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  placeholder={keptSecret ? 'Leave empty to keep the saved password' : 'Password'}
                  autoComplete="off"
                />
              )}
              {auth === 'key' && (
                <>
                  <textarea
                    className={clsx(inputClass, 'h-28 py-2 font-mono text-[12px] leading-snug')}
                    value={privateKey}
                    onChange={(e) => setPrivateKey(e.target.value)}
                    placeholder={keptSecret ? 'Leave empty to keep the saved key' : '-----BEGIN OPENSSH PRIVATE KEY-----'}
                    autoCapitalize="none"
                    autoCorrect="off"
                    spellCheck={false}
                  />
                  <input type="password" className={inputClass} value={passphrase} onChange={(e) => setPassphrase(e.target.value)} placeholder="Passphrase, if the key has one" autoComplete="off" />
                </>
              )}
            </>
          )}
          </>
        )}

        {asking && found && (
          <div ref={fingerprint} className={clsx('flex scroll-mb-4 gap-3 rounded-2xl px-3.5 py-3 text-[13px] leading-relaxed', changed ? 'bg-danger-soft text-danger' : 'bg-sunken text-fg-2')}>
            {changed ? <ShieldAlert className="mt-0.5 size-4 shrink-0" /> : <ShieldCheck className="mt-0.5 size-4 shrink-0 text-fg-2" />}
            <span className="min-w-0">
              {changed ? "The server's key is different from the one you trusted before. Only continue if you reinstalled it or changed its keys." : 'The server says its key is:'}
              <code className="mt-1.5 block break-all font-mono text-[12px] text-fg">{found.fingerprint}</code>
              <span className="mt-1 block text-[12px] text-fg-3">
                {found.keyType}. To compare, run <code className="font-mono">ssh-keygen -lf /etc/ssh/ssh_host_{keyName(found.keyType)}_key.pub</code> on the server.
              </span>
            </span>
          </div>
        )}
        {onLaptop && (
          <button type="button" onClick={onLaptop} className="py-1.5 text-[12.5px] text-fg-3 underline-offset-2 hover:text-fg hover:underline">
            Scan a code instead (easiest)
          </button>
        )}
        {edit && known && !asking && !computer && (
          <button type="button" onClick={check} className="-my-1.5 py-1.5 text-[12.5px] text-fg-3 underline-offset-2 hover:text-fg hover:underline">
            Check the server's key again
          </button>
        )}
      </form>
    </Sheet>
  )
}

/** "ssh-ed25519" → "ed25519", the name of its host key file. */
const keyName = (type: string) => (type.includes('ed25519') ? 'ed25519' : type.includes('ecdsa') ? 'ecdsa' : 'rsa')
