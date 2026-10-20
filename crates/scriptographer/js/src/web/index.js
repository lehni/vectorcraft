// What scripts see on the web that's written in JavaScript. The runtime passes
// what it needs as `__scriptographer`, which we take off the global object.
import { LocalFile } from './local-file.js'
import { setupPickers } from './pickers.js'

const { ask } = globalThis.__scriptographer
delete globalThis.__scriptographer

setupPickers(ask)
globalThis.LocalFile = LocalFile
