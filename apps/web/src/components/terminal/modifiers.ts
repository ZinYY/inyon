export type Modifiers = { ctrl: boolean; alt: boolean; shift: boolean }

/** Touch modifiers use the same xterm key sequences as a physical keyboard. Shift stays latched. */
export function modifiedInput(data: string, mods: Modifiers): { data: string; mods: Modifiers } {
  const bits = Number(mods.shift) | (Number(mods.alt) << 1) | (Number(mods.ctrl) << 2)
  if (!bits || data.startsWith('\x1b[200~')) return { data, mods }
  let output = data
  const csi = /^\x1b\[(\d*)(?:;(\d+))?([ABCDHF~])$/.exec(data)
  const ss3 = /^\x1bO([ABCDHFPQRS])$/.exec(data)
  if (csi) output = `\x1b[${csi[1] || 1};${((Number(csi[2] || 1) - 1) | bits) + 1}${csi[3]}`
  else if (ss3) output = `\x1b[1;${bits + 1}${ss3[1]}`
  else if (data === '\t' && mods.shift) output = bits === 1 ? '\x1b[Z' : `\x1b[9;${bits + 1}u`
  else if (data === '\r' && mods.shift) output = `\x1b[13;${bits + 1}u`
  else if (data.length === 1) {
    if (mods.shift) {
      const plain = "`1234567890-=[]\\;',./"
      const shifted = '~!@#$%^&*()_+{}|:"<>?'
      const at = plain.indexOf(data)
      output = at >= 0 ? shifted[at] : data.toUpperCase()
    }
    if (mods.ctrl) {
      const code = output.toUpperCase().charCodeAt(0)
      if (code >= 64 && code <= 95) output = String.fromCharCode(code - 64)
    }
    if (mods.alt) output = `\x1b${output}`
  } else return { data, mods }
  return { data: output, mods: { ctrl: false, alt: false, shift: mods.shift } }
}
