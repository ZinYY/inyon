import { useEffect, useState } from 'react'
import { api } from '../lib/api.ts'
import { useStore } from '../lib/store.ts'
import { Logo } from '../components/AppShell.tsx'
import { Button, Field, inputClass } from '../components/ui/primitives.tsx'
import { isPhone, phone } from '../lib/phone.ts'

export function Login() {
  // the phone app has nothing to sign in to; it only lands here when its core couldn't start
  if (isPhone()) return <PhoneFailed />
  return <SignIn />
}

function PhoneFailed() {
  const [error, setError] = useState('')
  useEffect(() => {
    phone().then(() => window.location.reload(), (err) => setError(String((err as Error)?.message ?? err)))
  }, [])
  return (
    <div className="flex min-h-dvh items-center justify-center px-6">
      <div className="w-full max-w-[360px]">
        <Logo className="size-12 rounded-2xl" target />
        <h1 className="mt-6 text-[32px] leading-[0.95] font-semibold tracking-[-0.04em]">Inyon couldn't start</h1>
        <p className="mt-1.5 text-[15px] text-fg-2">{error || 'Starting…'}</p>
        <Button variant="primary" size="lg" className="mt-6 w-full" onClick={() => window.location.reload()}>
          Try again
        </Button>
      </div>
    </div>
  )
}

function SignIn() {
  const [password, setPassword] = useState('')
  const [confirm, setConfirm] = useState('')
  const [code, setCode] = useState('')
  // a new hub has no password yet: the first visit creates it
  const [setup, setSetup] = useState<{ code: boolean } | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const setAuthed = useStore((s) => s.setAuthed)

  useEffect(() => {
    api.session().then((s) => s.setup && setSetup({ code: !!s.setupCode })).catch(() => {})
  }, [])

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    setBusy(true)
    setError('')
    try {
      if (setup) {
        if (password.length < 8) throw new Error('Use at least 8 characters.')
        if (password !== confirm) throw new Error("The two passwords don't match.")
        await api.createPassword(password, setup.code ? code : undefined)
      } else await api.login(password)
      setAuthed(true)
    } catch (err) {
      setError((err as Error).message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="flex min-h-dvh items-center justify-center px-6">
      <form onSubmit={submit} className="animate-slide-up w-full max-w-[360px]">
        <Logo className="size-12 rounded-2xl" target />
        <h1 className="mt-6 text-[32px] leading-[0.95] font-semibold tracking-[-0.04em]">{setup ? 'Create your password' : 'Welcome back'}</h1>
        <p className="mt-1.5 text-[15px] text-fg-2">
          {setup ? "This hub is new. Pick the password you'll use to sign in from your browsers." : 'Sign in to reach your machines and coding agents.'}
        </p>
        <input
          type="password"
          autoFocus
          autoComplete={setup ? 'new-password' : 'current-password'}
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          placeholder={setup ? 'New password (8+ characters)' : 'Password'}
          aria-label={setup ? 'New password' : 'Password'}
          className={`${inputClass} mt-7 h-12`}
        />
        {setup && (
          <input
            type="password"
            autoComplete="new-password"
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
            placeholder="Type it again"
            aria-label="Type the password again"
            className={`${inputClass} mt-3 h-12`}
          />
        )}
        {setup?.code && (
          <div className="mt-4">
            <Field label="Setup code" hint="The hub printed it when it started. On the hub machine, `inyon hub code` shows it again.">
              <input
                value={code}
                onChange={(e) => setCode(e.target.value)}
                autoCapitalize="none"
                autoCorrect="off"
                spellCheck={false}
                className={`${inputClass} h-12 font-mono text-[14px]`}
              />
            </Field>
          </div>
        )}
        {error && <p className="mt-2 text-sm text-danger">{error}</p>}
        <Button type="submit" variant="primary" size="lg" className="mt-4 w-full" loading={busy} disabled={!password || (!!setup && !confirm) || (!!setup?.code && !code.trim())}>
          {setup ? 'Create password' : 'Continue'}
        </Button>
      </form>
    </div>
  )
}
