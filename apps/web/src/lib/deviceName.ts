import { plugin } from './native.ts'

/** The model from an Android user agent, without the build: "A142" from "…; A142 Build/BP2A.2…; wv)". */
export function modelFromUserAgent(ua: string): string | undefined {
  const m = /Android [\d.]+; ([^;)]+)/.exec(ua)?.[1]?.replace(/\s+Build\/.*$/, '').trim()
  return m && m !== 'K' && m !== 'wv' ? m : undefined
}

/** "Nothing A142": the maker, then the model, unless the model already starts with the maker. */
export function readableName(manufacturer?: string, model?: string): string | undefined {
  const maker = (manufacturer ?? '').trim()
  const name = (model ?? '').trim()
  if (!name) return maker || undefined
  if (!maker || name.toLowerCase().startsWith(maker.toLowerCase())) return name
  return `${maker[0]!.toUpperCase()}${maker.slice(1)} ${name}`
}

/**
 * This phone's name as a computer or hub will list it: the maker and model from Android ("Nothing
 * A142"), the model from the browser's user agent otherwise, or `fallback`.
 */
export async function deviceName(fallback = 'Phone'): Promise<string> {
  const device = plugin('InyonDevice')
  if (typeof device?.deviceName === 'function') {
    try {
      const r = await device.deviceName()
      const name = readableName(r?.manufacturer, r?.model)
      if (name) return name
    } catch {}
  }
  return (typeof navigator !== 'undefined' && modelFromUserAgent(navigator.userAgent)) || fallback
}
