import { EventTarget, Event, defineEventHandlers, fire } from './events.js'
import { setToStringTag } from './define.js'

// `AbortController` and `AbortSignal`, for scripts outside a browser.

// Only the controller and the static methods make signals:
const internal = Symbol('internal')
let abort

export class AbortSignal extends EventTarget {
  #aborted = false
  #reason

  constructor(key) {
    if (key !== internal) {
      throw new TypeError(
        'Invalid AbortSignal: AbortController, AbortSignal.abort() and ' +
        'AbortSignal.timeout() make them.'
      )
    }
    super()
  }

  static {
    // An `abort` event for `onabort` and the listeners, once.
    abort = (signal, reason) => {
      if (!signal.#aborted) {
        signal.#aborted = true
        signal.#reason =
          reason === undefined
            ? new DOMException(
                'signal is aborted without reason',
                'AbortError'
              )
            : reason
        fire(signal, new Event('abort'))
      }
    }
  }

  get aborted() {
    return this.#aborted
  }

  get reason() {
    return this.#reason
  }

  throwIfAborted() {
    if (this.#aborted) throw this.#reason
  }

  static abort(reason) {
    const signal = new AbortSignal(internal)
    abort(signal, reason)
    return signal
  }

  static timeout(milliseconds) {
    const signal = new AbortSignal(internal)
    setTimeout(
      () => abort(signal, new DOMException('signal timed out', 'TimeoutError')),
      milliseconds
    )
    return signal
  }

  // A signal that aborts when the first of `signals` does.
  static any(signals) {
    const signal = new AbortSignal(internal)
    for (const from of signals) {
      if (from.aborted) {
        abort(signal, from.reason)
        break
      }
      from.addEventListener('abort', () => abort(signal, from.reason), {
        once: true
      })
    }
    return signal
  }
}

export class AbortController {
  #signal = new AbortSignal(internal)

  get signal() {
    return this.#signal
  }

  abort(reason) {
    abort(this.#signal, reason)
  }
}

defineEventHandlers(AbortSignal.prototype, 'abort')
setToStringTag(AbortSignal, AbortController)
