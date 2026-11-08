/** Normalize the TeX delimiters emitted by agents without touching code. */
export function mathDelimiters(text: string): string {
  const prose = (value: string) => value.replace(/(`+)[\s\S]*?\1|\\\\|\\\(([\s\S]*?)\\\)|\\\[([\s\S]*?)\\\]/g,
    (all, code, inline, block) => code || all === '\\\\' ? all : inline !== undefined ? `$${inline}$` : block !== undefined ? `\n$$\n${block.trim()}\n$$\n` : all)
  let result = '', pending = '', fence = '', length = 0
  for (const line of text.match(/[^\n]*\n|[^\n]+$/g) ?? []) {
    const mark = line.match(/^\s{0,3}(`{3,}|~{3,})/)
    if (fence) {
      result += line
      if (mark?.[1]?.[0] === fence && mark[1].length >= length && line.slice(mark[0].length).trim() === '') fence = ''
    } else if (mark || /^(?: {4}|\t)/.test(line)) {
      result += prose(pending) + line; pending = ''
      if (mark) { fence = mark[1]![0]!; length = mark[1]!.length }
    } else pending += line
  }
  return result + prose(pending)
}
