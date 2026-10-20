// What scripts see that's written in JavaScript, on every platform. The runtime
// runs it before its own classes, which build on these.
import { setupFileDialogs } from './dialog.js'
import * as events from './events.js'
import * as geometry from './geometry.js'

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
