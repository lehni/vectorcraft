import {
  checkArguments,
  checkInternal,
  internal,
  setToStringTag
} from './define.js'
import {
  EventTarget,
  MessageEvent,
  defineEventHandlers,
  fire
} from './events.js'
import { natives } from './natives.js'

// `MessageChannel` and `MessagePort` for scripts outside a browser: the runtime
// joins the two ports of a channel, in one runtime or, once one is sent to a
// worker, in two, and hands each port what arrived on the script's turn, once
// it started.

let getNumber
let detach

export class MessageChannel {
  #port1
  #port2

  constructor() {
    const [first, second] = natives.port.channel()
    this.#port1 = new MessagePort(internal, first)
    this.#port2 = new MessagePort(internal, second)
  }

  get port1() {
    return this.#port1
  }

  get port2() {
    return this.#port2
  }
}

export class MessagePort extends EventTarget {
  // `null` once the port is closed or sent:
  #number

  constructor(key, number) {
    checkInternal(key, 'MessagePort', 'MessageChannel makes them')
    super()
    this.#number = number
  }

  static {
    getNumber = port => port.#number
    detach = port => {
      port.#number = null
    }
  }

  postMessage(message, transfer) {
    checkArguments(arguments, 1, 'postMessage')
    postWith(transfer, (ports, numbers) => {
      if (this.#number !== null) {
        if (ports.includes(this)) {
          throw new DOMException(
            "Invalid postMessage(): a port can't transfer itself.",
            'DataCloneError'
          )
        }
        natives.port.post(this.#number, message, ports, numbers)
      }
    })
  }

  start() {
    if (this.#number !== null) {
      natives.port.start(this.#number, ({ type, data, ports }) =>
        fire(this, new MessageEvent(type, { data, ports }))
      )
    }
  }

  close() {
    if (this.#number !== null) {
      natives.port.close(this.#number)
      this.#number = null
    }
  }
}

defineEventHandlers(MessagePort.prototype, 'message', 'messageerror')
// Setting `onmessage` starts the port, as in browsers:
const { set } = Object.getOwnPropertyDescriptor(
  MessagePort.prototype,
  'onmessage'
)
Object.defineProperty(MessagePort.prototype, 'onmessage', {
  set(handler) {
    set.call(this, handler)
    this.start()
  }
})
setToStringTag(MessageChannel, MessagePort)

// Sends a message with the ports in `transfer`, a list of them or `{ transfer }`
// as `postMessage()` takes it: `send(ports, numbers)` sends, and once it did,
// the ports are the receiver's.
export function postWith(transfer, send) {
  const list =
    transfer === undefined || transfer === null
      ? []
      : typeof transfer === 'object' && Symbol.iterator in transfer
        ? [...transfer]
        : typeof transfer === 'object'
          ? [...(transfer.transfer ?? [])]
          : null
  if (!list) {
    throw new TypeError(`Invalid transfer list: '${transfer}'.`)
  }
  for (const [index, port] of list.entries()) {
    if (
      !(port instanceof MessagePort) ||
      getNumber(port) === null ||
      list.indexOf(port) !== index
    ) {
      throw new DOMException(
        `Invalid transfer list: '${port}' can't be transferred.`,
        'DataCloneError'
      )
    }
  }
  send(list, list.map(getNumber))
  list.forEach(detach)
}

// A port the runtime received in a message, by its number:
export function makePort(number) {
  return new MessagePort(internal, number)
}
