import { inject, setToStringTag } from './define.js'
import {
  ErrorEvent,
  EventTarget,
  MessageEvent,
  defineEventHandlers,
  fire
} from './events.js'
import { natives } from './natives.js'

// `Worker` for scripts outside a browser, and what a worker's global object
// has. The runtime runs each worker's script on a thread of its own, in a
// runtime of its own, and passes the messages both ways as structured
// serialization keeps them. Module workers aren't supported.

export class Worker extends EventTarget {
  #number

  constructor(url, { type = 'classic', name = '' } = {}) {
    super()
    if (type === 'module') {
      throw new DOMException(
        'Unsupported Worker: module workers are not supported.',
        'NotSupportedError'
      )
    }
    this.#number = natives.worker.start(String(url), String(name), sent =>
      this.#receive(sent)
    )
  }

  postMessage(data) {
    natives.worker.post(this.#number, data)
  }

  terminate() {
    natives.worker.terminate(this.#number)
  }

  // What the worker sent: `{ type, data }` or `{ type: 'error', message }`.
  #receive({ type, data, message }) {
    if (type === 'error') {
      // Unless a handler prevents it, the error shows in the console too, as in
      // browsers.
      const event = new ErrorEvent('error', { message, cancelable: true })
      if (fire(this, event)) {
        console.error(`Uncaught (in worker) ${message}`)
      }
    } else {
      fire(this, new MessageEvent(type, { data }))
    }
  }
}

defineEventHandlers(Worker.prototype, 'message', 'messageerror', 'error')
setToStringTag(Worker)

// Give the worker's global object what a worker's has: `self` is an
// `EventTarget`, with `name`, `postMessage()`, `close()` and `importScripts()`.
// What the parent sends, the runtime hands to the function this returns.
export function setupWorker(name) {
  const { addEventListener, removeEventListener, dispatchEvent } =
    EventTarget.prototype
  const { postMessage, close, importScripts } = natives.worker.self
  inject(globalThis, {
    name,
    postMessage,
    close,
    importScripts,
    addEventListener,
    removeEventListener,
    dispatchEvent
  })
  defineEventHandlers(globalThis, 'message', 'messageerror')
  return ({ type, data }) => fire(globalThis, new MessageEvent(type, { data }))
}
