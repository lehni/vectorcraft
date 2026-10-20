// What scripts see that's written in JavaScript, on every platform. The runtime
// runs it before its own classes, which build on these.
import { createConsole } from './console.js'
import { setupFileDialogs } from './dialog.js'
import * as events from './events.js'
import * as geometry from './geometry.js'

// What the runtime passes, which we take off the global object:
const { print } = globalThis.__scriptographer
delete globalThis.__scriptographer

// Its own console, also in a browser's realm, for the script's output:
const console = createConsole(print)
setGlobals({ console, print: console.log })

// What a browser's realm has already, it keeps:
if (!globalThis.EventTarget) {
  setGlobals(events)
}
if (!globalThis.DOMMatrix) {
  setGlobals({ ...geometry, WebKitCSSMatrix: geometry.DOMMatrix })
}
setupFileDialogs((globalThis.Dialog ??= {}))

// Set as the realm's own globals are: not enumerable.
function setGlobals(values) {
  for (const [name, value] of Object.entries(values)) {
    Object.defineProperty(globalThis, name, {
      value,
      writable: true,
      configurable: true
    })
  }
}
