import { VERSION } from '@inyon/protocol'

declare const __INYON_COMMIT__: string

/** The commit this build came from (set by Vite; empty in tests and unbuilt runs). */
export const COMMIT = typeof __INYON_COMMIT__ === 'string' ? __INYON_COMMIT__ : ''

export { VERSION }
