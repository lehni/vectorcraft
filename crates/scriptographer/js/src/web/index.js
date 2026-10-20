// What scripts see on the web that's written in JavaScript. The runtime passes
// what it needs as `__scriptographer`, which the bundle for every platform takes
// off the global object after this one.
import { LocalFile } from './local-file.js'
import { setupPickers } from './pickers.js'

const { ask } = globalThis.__scriptographer

setupPickers(ask)
globalThis.LocalFile = LocalFile
