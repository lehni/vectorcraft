import { reportError } from './global-scope.js'
import { defineConstants, inject, setToStringTag } from './define.js'

// `EventTarget` and the event classes for scripts outside a browser, as the DOM
// standard has them, to extend (`class Emitter extends EventTarget`). There's
// no tree to propagate through: an event reaches its target only.

const phases = { NONE: 0, CAPTURING_PHASE: 1, AT_TARGET: 2, BUBBLING_PHASE: 3 }

// A target's listeners, and an event's state, out of their own properties, so
// any object that adopts the prototypes (`AbortSignal`) has them:
const listenersByTarget = new WeakMap()
const states = new WeakMap()
// The `on…` handlers of targets, by the event types they handle:
const handlersByTarget = new WeakMap()

export class EventTarget {
  addEventListener(type, callback, options) {
    if (
      callback && typeof callback === 'object' ||
      typeof callback === 'function'
    ) {
      const { capture, once, passive, signal } = getOptions(options)
      if (!signal?.aborted) {
        type = String(type)
        let listeners = listenersByTarget.get(this)
        if (!listeners) {
          listeners = []
          listenersByTarget.set(this, listeners)
        }
        if (!find(listeners, type, callback, capture)) {
          const listener = {
            type,
            callback,
            capture,
            once,
            passive,
            removed: false
          }
          listeners.push(listener)
          signal?.addEventListener('abort', () => remove(this, listener))
        }
      }
    }
  }

  removeEventListener(type, callback, options) {
    const { capture } = getOptions(options)
    const listener = find(
      listenersByTarget.get(this),
      String(type),
      callback,
      capture
    )
    if (listener) remove(this, listener)
  }

  dispatchEvent(event) {
    const state = states.get(event)
    if (!state) {
      throw new TypeError(`Invalid event: '${event}', not an Event.`)
    }
    if (state.dispatching) {
      throw new DOMException(
        'Unable to dispatch an event that is being dispatched.',
        'InvalidStateError'
      )
    }
    Object.assign(state, {
      dispatching: true,
      target: this,
      currentTarget: this,
      eventPhase: phases.AT_TARGET
    })
    // Those added meanwhile wait for the next event; those removed are skipped.
    for (const listener of [...(listenersByTarget.get(this) ?? [])]) {
      if (!listener.removed && listener.type === state.type) {
        if (listener.once) remove(this, listener)
        const { callback } = listener
        state.passive = listener.passive
        try {
          if (typeof callback === 'function') {
            callback.call(this, event)
          } else if (typeof callback.handleEvent === 'function') {
            callback.handleEvent(event)
          } else {
            throw new TypeError('Invalid listener: it has no handleEvent().')
          }
        } catch (err) {
          // A listener that throws is reported, and the others run:
          reportError(err)
        }
        state.passive = false
        if (state.stopImmediately) break
      }
    }
    Object.assign(state, {
      dispatching: false,
      stop: false,
      stopImmediately: false,
      currentTarget: null,
      eventPhase: phases.NONE
    })
    return !state.defaultPrevented
  }
}

export class Event {
  constructor(type, init) {
    if (arguments.length === 0) {
      throw new TypeError('Invalid event: its type is missing.')
    }
    const { bubbles, cancelable, composed } = init ?? {}
    states.set(this, {
      type: String(type),
      bubbles: !!bubbles,
      cancelable: !!cancelable,
      composed: !!composed,
      defaultPrevented: false,
      eventPhase: phases.NONE,
      target: null,
      currentTarget: null,
      timeStamp: globalThis.performance?.now() ?? 0,
      isTrusted: false
    })
  }

  get srcElement() {
    return getState(this).target
  }

  get returnValue() {
    return !getState(this).defaultPrevented
  }

  set returnValue(value) {
    if (!value) this.preventDefault()
  }

  get cancelBubble() {
    return !!getState(this).stop
  }

  set cancelBubble(value) {
    if (value) getState(this).stop = true
  }

  // A passive listener can't cancel an event:
  preventDefault() {
    const state = getState(this)
    if (state.cancelable && !state.passive) state.defaultPrevented = true
  }

  stopPropagation() {
    getState(this).stop = true
  }

  stopImmediatePropagation() {
    Object.assign(getState(this), { stop: true, stopImmediately: true })
  }

  // The target while the event is dispatched, as there's no tree around it.
  composedPath() {
    const { currentTarget } = getState(this)
    return currentTarget ? [currentTarget] : []
  }

  // The legacy way, before the event is dispatched.
  initEvent(type, bubbles = false, cancelable = false) {
    const state = getState(this)
    if (!state.dispatching) {
      Object.assign(state, {
        type: String(type),
        bubbles: !!bubbles,
        cancelable: !!cancelable,
        stop: false,
        stopImmediately: false,
        defaultPrevented: false
      })
    }
  }
}

// The subclasses, with their init dictionaries' members and what they are
// without them:
export class CustomEvent extends Event {
  constructor(type, init) {
    super(...arguments)
    setMembers(this, init, { detail: null })
  }

  initCustomEvent(type, bubbles, cancelable, detail = null) {
    this.initEvent(type, bubbles, cancelable)
    const state = getState(this)
    if (!state.dispatching) state.detail = detail
  }
}

export class MessageEvent extends Event {
  constructor(type, init) {
    super(...arguments)
    setMembers(this, init, {
      data: null,
      origin: '',
      lastEventId: '',
      source: null,
      ports: []
    })
  }
}

export class CloseEvent extends Event {
  constructor(type, init) {
    super(...arguments)
    setMembers(this, init, { wasClean: false, code: 0, reason: '' })
  }
}

export class ProgressEvent extends Event {
  constructor(type, init) {
    super(...arguments)
    setMembers(this, init, { lengthComputable: false, loaded: 0, total: 0 })
  }
}

export class ErrorEvent extends Event {
  constructor(type, init) {
    super(...arguments)
    setMembers(this, init, {
      message: '',
      filename: '',
      lineno: 0,
      colno: 0,
      error: null
    })
  }
}

// What every event has, and the subclasses' own, read-only:
const fields = {
  [Event.name]: [
    'type',
    'bubbles',
    'cancelable',
    'composed',
    'defaultPrevented',
    'eventPhase',
    'target',
    'currentTarget',
    'timeStamp',
    'isTrusted'
  ],
  [CustomEvent.name]: ['detail'],
  [MessageEvent.name]: ['data', 'origin', 'lastEventId', 'source', 'ports'],
  [CloseEvent.name]: ['wasClean', 'code', 'reason'],
  [ProgressEvent.name]: ['lengthComputable', 'loaded', 'total'],
  [ErrorEvent.name]: ['message', 'filename', 'lineno', 'colno', 'error']
}

for (const Class of [
  Event,
  CustomEvent,
  MessageEvent,
  CloseEvent,
  ProgressEvent,
  ErrorEvent
]) {
  for (const name of fields[Class.name]) {
    inject(
      Class.prototype,
      {
        get [name]() {
          return getState(this)[name]
        }
      },
      { enumerable: true }
    )
  }
}

defineConstants(Event, phases)

setToStringTag(
  EventTarget,
  Event,
  CustomEvent,
  MessageEvent,
  CloseEvent,
  ProgressEvent,
  ErrorEvent
)

// The `on…` event handlers of `target`, a prototype or the global object, for
// the event `types`.
export function defineEventHandlers(target, ...types) {
  for (const type of types) {
    inject(
      target,
      {
        get [`on${type}`]() {
          return handlersByTarget.get(this)?.[type] ?? null
        },

        set [`on${type}`](handler) {
          let handlers = handlersByTarget.get(this)
          if (!handlers) {
            handlers = {}
            handlersByTarget.set(this, handlers)
          }
          handlers[type] = typeof handler === 'function' ? handler : null
        }
      },
      { enumerable: true }
    )
  }
}

// Fire `event` at `target`: to its `on…` handler for the event's type, then to
// its listeners. Whether nothing prevented its default.
export function fire(target, event) {
  const handler = handlersByTarget.get(target)?.[event.type]
  if (handler) {
    try {
      handler.call(target, event)
    } catch (err) {
      reportError(err)
    }
  }
  return target.dispatchEvent(event)
}

function getState(event) {
  const state = states.get(event)
  if (!state) {
    throw new TypeError(`Invalid event: '${event}', not an Event.`)
  }
  return state
}

// The members of a subclass's init dictionary, converted as its fields are.
function setMembers(event, init, defaults) {
  const state = getState(event)
  for (const [name, value] of Object.entries(defaults)) {
    const given = init?.[name]
    state[name] =
      given === undefined
        ? Array.isArray(value)
          ? []
          : value
        : typeof value === 'string'
          ? String(given)
          : typeof value === 'number'
            ? Number(given)
            : typeof value === 'boolean'
              ? !!given
              : given
  }
}

// The listener options: `capture`, from a boolean too, and for adding, `once`,
// `passive` and `signal`.
function getOptions(options) {
  if (options && typeof options === 'object') {
    const { capture, once, passive, signal } = options
    return { capture: !!capture, once: !!once, passive: !!passive, signal }
  } else {
    return { capture: !!options, once: false, passive: false, signal: null }
  }
}

function find(listeners, type, callback, capture) {
  return listeners?.find(
    listener => (
      listener.type === type &&
      listener.capture === capture &&
      Object.is(listener.callback, callback)
    )
  )
}

// Takes `listener` off its target's list, so a dispatch under way skips it too.
function remove(target, listener) {
  listener.removed = true
  const listeners = listenersByTarget.get(target)
  const index = listeners?.indexOf(listener) ?? -1
  if (index >= 0) listeners.splice(index, 1)
}
