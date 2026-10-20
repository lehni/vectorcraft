import { natives } from './natives.js'

// `window` and `document` for scripts outside a browser: linkedom's, which the
// runtime loads the first time a script reads either one.

let view

export const dom = {
  get window() {
    return (view ??= natives.dom())
  },

  get document() {
    return (view ??= natives.dom()).document
  }
}
