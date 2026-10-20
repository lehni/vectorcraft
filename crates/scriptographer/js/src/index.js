// What scripts see that's written in JavaScript, on every platform. The runtime
// runs it before its own classes, which build on these.
import { natives } from './natives.js'
import { inject } from './define.js'
import * as abort from './abort.js'
import * as blob from './blob.js'
import * as canvas from './canvas.js'
import { Clipboard } from './clipboard.js'
import { createConsole } from './console.js'
import { Crypto } from './crypto.js'
import { setupDialog } from './dialog.js'
import { DOMException } from './dom-exception.js'
import { TextDecoder, TextEncoder } from './encoding.js'
import * as events from './events.js'
import * as fetch from './fetch.js'
import * as fileSystem from './file-system.js'
import * as geometry from './geometry.js'
import * as globalScope from './global-scope.js'
import { LocalFile } from './local-file.js'
import {
  Palette,
  handlePaletteEvent,
  hasPalettes,
  takePalettes
} from './palette.js'
import { Storage, createStorage } from './storage.js'
import { URL, URLSearchParams } from './url.js'
import { WebSocket } from './websocket.js'
import { Worker, setupWorker } from './worker.js'

delete globalThis.__scriptographer
const { print } = natives

// Its own console, also in a browser's realm, for the script's output:
const console = createConsole(print)
inject(globalThis, { console, print: console.log })

// What a browser's realm has already, it keeps:
inject(
  globalThis,
  Object.fromEntries(
    Object.entries(globalScope).filter(([name]) => !(name in globalThis))
  )
)
if (globalThis.navigator && !navigator.clipboard) {
  inject(globalThis, { Clipboard })
  inject(navigator, { clipboard: new Clipboard() }, { writable: false })
}
if (!globalThis.TextEncoder) {
  inject(globalThis, { TextEncoder, TextDecoder })
}
if (!globalThis.crypto) {
  inject(globalThis, { Crypto, crypto: new Crypto() })
}
if (!globalThis.DOMException) {
  inject(globalThis, { DOMException })
}
if (!globalThis.EventTarget) {
  inject(globalThis, { ...events })
}
if (!globalThis.fetch) {
  const { Blob, File, getBytes } = blob
  const { getBody, ...classes } = fetch
  inject(globalThis, { ...abort, Blob, File, ...classes })
  // What the runtime reads of blobs:
  Object.assign(natives, { getBytes })
}
if (!globalThis.localStorage) {
  inject(globalThis, {
    Storage,
    localStorage: createStorage(false),
    sessionStorage: createStorage(true)
  })
}
if (!globalThis.URL) {
  inject(globalThis, { URL, URLSearchParams })
}
if (!globalThis.WebSocket) {
  inject(globalThis, { WebSocket })
}
if (!globalThis.Worker) {
  inject(globalThis, { Worker })
  // What the runtime makes a worker's global object with:
  Object.assign(natives, { setupWorker })
}
// The file system on desktop, by path; the web has the browser's:
if (natives.fs) {
  const { getHandlePath, ...classes } = fileSystem
  inject(globalThis, { LocalFile, ...classes })
}
if (!globalThis.OffscreenCanvas) {
  inject(globalThis, { ...canvas })
}
if (!globalThis.DOMMatrix) {
  inject(globalThis, { ...geometry, WebKitCSSMatrix: geometry.DOMMatrix })
}
setupDialog((globalThis.Dialog ??= {}))
inject(globalThis, { Palette })
// What the runtime reports of the palettes, and tells them:
Object.assign(natives, { takePalettes, hasPalettes, handlePaletteEvent })
