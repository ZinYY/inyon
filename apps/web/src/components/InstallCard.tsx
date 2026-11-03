import { useState } from 'react'
import clsx from 'clsx'
import { Share, X } from 'lucide-react'
import { dismissInstall, install, installDismissed, useInstallWay } from '../lib/install.ts'
import { Button } from './ui/primitives.tsx'
import { LEAVE_MS } from './ui/AnimatedList.tsx'

/**
 * The web app, once a computer is paired: install it, so the browser keeps its data and it opens
 * like an app. On iPhone that matters most, since Safari clears a site's data after 7 days
 * without a visit, and the Home Screen app keeps its own data apart from Safari's.
 */
export function InstallCard() {
  const way = useInstallWay()
  const [gone, setGone] = useState(installDismissed)
  const [closing, setClosing] = useState(false)
  if (!way || gone) return null
  // remembered at once; the card fades for a moment before it goes
  const close = () => {
    dismissInstall()
    setClosing(true)
    setTimeout(() => setGone(true), LEAVE_MS)
  }
  return (
    <div className={clsx('relative rounded-3xl border border-line bg-elev p-5 pr-12 shadow-card', closing ? 'animate-fade-out' : 'animate-fade-in')}>
      <p className="text-[15px] font-semibold tracking-tight">{way === 'share' ? 'Add Inyon to your Home Screen' : 'Install Inyon'}</p>
      {way === 'share' ? (
        <p className="mt-1.5 text-[13.5px] leading-relaxed text-fg-2">
          Safari forgets this page after 7 days unused. Tap <Share className="inline size-3.5 align-[-2px]" aria-label="Share" /> Share, then Add to Home Screen, and pair once more there with <code className="font-mono text-[12.5px]">inyon pair</code>.
        </p>
      ) : (
        <>
          <p className="mt-1.5 text-[13.5px] leading-relaxed text-fg-2">It opens like an app and keeps your computers paired.</p>
          <Button size="sm" variant="primary" className="mt-3" onClick={() => void install()}>
            Install
          </Button>
        </>
      )}
      <button type="button" aria-label="Not now" title="Not now" onClick={close} className="press absolute top-4 right-4 flex size-8 items-center justify-center rounded-full text-fg-3 hover:bg-hover hover:text-fg after:absolute after:-inset-1">
        <X className="size-4" />
      </button>
    </div>
  )
}
