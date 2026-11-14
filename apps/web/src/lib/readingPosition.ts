export interface ReadingPosition {
  top: number
  follow: boolean
  anchors: { id: string; top: number }[]
}

const positions = new Map<string, ReadingPosition>()
export const forgetReadingPositions = () => positions.clear()
export const readingPosition = (key: string) => positions.get(key)
export function rememberReadingPosition(key: string, position: ReadingPosition) {
  positions.delete(key)
  positions.set(key, position)
  if (positions.size > 64) positions.delete(positions.keys().next().value!)
}
export function inheritReadingPosition(from: string, to: string) {
  const position = positions.get(from)
  if (position) rememberReadingPosition(to, position)
}
