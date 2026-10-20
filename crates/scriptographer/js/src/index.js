// What scripts see that's written in JavaScript, on every platform. The runtime
// runs it before its own classes, which build on these.
import { natives } from './natives.js'
import { defineValues } from './define.js'
import * as abort from './abort.js'
import * as blob from './blob.js'
import { createConsole } from './console.js'
import { setupFileDialogs } from './dialog.js'
import { DOMException } from './dom-exception.js'
import * as events from './events.js'
import * as fetch from './fetch.js'
import * as geometry from './geometry.js'
import { Storage, createStorage } from './storage.js'
import { URL, URLSearchParams } from './url.js'
import { WebSocket } from './websocket.js'
import { Worker, setupWorker } from './worker.js'

delete globalThis.__scriptographer
const { print } = natives

// Its own console, also in a browser's realm, for the script's output:
const console = createConsole(print)
setGlobals({ console, print: console.log })

// What a browser's realm has already, it keeps:
if (!globalThis.DOMException) {
  setGlobals({ DOMException })
}
if (!globalThis.EventTarget) {
  setGlobals(events)
}
if (!globalThis.fetch) {
  const { Blob, File, getBytes } = blob
  const { getBody, ...classes } = fetch
  setGlobals({ ...abort, Blob, File, ...classes })
  // What the runtime reads of blobs and bodies:
  Object.assign(natives, { getBytes, getBody })
}
if (!globalThis.localStorage) {
  setGlobals({
    Storage,
    localStorage: createStorage(false),
    sessionStorage: createStorage(true)
  })
}
if (!globalThis.URL) {
  setGlobals({ URL, URLSearchParams })
}
if (!globalThis.WebSocket) {
  setGlobals({ WebSocket })
}
if (!globalThis.Worker) {
  setGlobals({ Worker })
  // What the runtime makes a worker's global object with:
  Object.assign(natives, { setupWorker })
}
if (!globalThis.DOMMatrix) {
  setGlobals({ ...geometry, WebKitCSSMatrix: geometry.DOMMatrix })
}
setupFileDialogs((globalThis.Dialog ??= {}))

// Set as the realm's own globals are: not enumerable.
function setGlobals(values) {
  defineValues(globalThis, values, { writable: true, configurable: true })
}
