// @inyon/phone in a build made with VITE_INYON_PHONE_CORE=none (vite.config.ts): the copy of the app
// a hub serves, which never runs the phone core, so it's built without Rust's wasm tools.
import type { StartPhone } from '@inyon/phone'

export const startPhone: StartPhone = async () => {
  throw new Error('This build of the app has no phone core')
}
