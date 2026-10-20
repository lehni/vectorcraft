import { Blob, getBytes } from './blob.js'
import { defineValues, setToStringTag } from './define.js'
import {
  CloseEvent,
  Event,
  EventTarget,
  MessageEvent,
  defineEventHandlers,
  fire
} from './events.js'
import { natives } from './natives.js'

// `WebSocket` for scripts outside a browser. The runtime connects each on a
// thread of its own and hands back what it reads on the script's turn, as the
// events browsers fire. An open socket keeps the script running.

const states = { CONNECTING: 0, OPEN: 1, CLOSING: 2, CLOSED: 3 }
// As browsers have it: http and https mean ws and wss.
const schemes = {
  'ws:': 'ws:',
  'http:': 'ws:',
  'wss:': 'wss:',
  'https:': 'wss:'
}

export class WebSocket extends EventTarget {
  #url
  #number
  #readyState = states.CONNECTING
  #protocol = ''
  #binaryType = 'blob'

  constructor(url, protocols = []) {
    super()
    url = String(url)
    const parsed = URL.canParse(url) ? new URL(url) : null
    const scheme = schemes[parsed?.protocol]
    if (!scheme || parsed.hash) {
      throw new DOMException(`Invalid WebSocket URL: '${url}'.`, 'SyntaxError')
    }
    parsed.protocol = scheme
    this.#url = parsed.href
    this.#number = natives.socket.open(
      this.#url,
      typeof protocols === 'string'
        ? [protocols]
        : Array.from(protocols, String),
      incoming => this.#receive(incoming)
    )
  }

  get url() {
    return this.#url
  }

  get readyState() {
    return this.#readyState
  }

  get protocol() {
    return this.#protocol
  }

  get extensions() {
    return ''
  }

  get bufferedAmount() {
    return 0
  }

  get binaryType() {
    return this.#binaryType
  }

  set binaryType(type) {
    if (type === 'blob' || type === 'arraybuffer') this.#binaryType = type
  }

  // Text, or the bytes of a `Blob`, a buffer or a view. A closing or closed
  // socket sends nothing, as in browsers.
  send(data) {
    if (arguments.length < 1) {
      throw new TypeError('Invalid send(): the data is missing.')
    }
    if (this.#readyState === states.CONNECTING) {
      throw new DOMException(
        'Unable to send: the WebSocket is not open yet.',
        'InvalidStateError'
      )
    }
    if (this.#readyState === states.OPEN) {
      natives.socket.send(
        this.#number,
        data instanceof Blob
          ? getBytes(data)
          : data instanceof ArrayBuffer
            ? new Uint8Array(data)
            : ArrayBuffer.isView(data)
              ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength)
              : String(data)
      )
    }
  }

  // A code of 1000 or 3000 to 4999, a reason of 123 bytes at most.
  close(code, reason = '') {
    if (
      code !== undefined &&
      (code = Number(code)) !== 1000 &&
      !(code >= 3000 && code <= 4999)
    ) {
      throw new DOMException(
        `Invalid WebSocket close code: '${code}'.`,
        'InvalidAccessError'
      )
    }
    reason = String(reason)
    if (new TextEncoder().encode(reason).length > 123) {
      throw new DOMException(
        'Invalid WebSocket close reason: it is longer than 123 bytes.',
        'SyntaxError'
      )
    }
    if (this.#readyState < states.CLOSING) {
      this.#readyState = states.CLOSING
      natives.socket.close(this.#number, code ?? 1000, reason)
    }
  }

  // What the socket's thread read: `{ type, … }`.
  #receive({ type, protocol, data, message, code, reason, wasClean }) {
    switch (type) {
      case 'open':
        this.#readyState = states.OPEN
        this.#protocol = protocol
        fire(this, new Event('open'))
        break
      case 'message':
        fire(
          this,
          new MessageEvent('message', {
            data:
              typeof data === 'string'
                ? data
                : this.#binaryType === 'arraybuffer'
                  ? data.buffer
                  : new Blob([data]),
            origin: new URL(this.#url).origin
          })
        )
        break
      case 'error':
        // As in browsers, the event says nothing of why; the console does.
        console.error(`WebSocket ${this.#url}: ${message}`)
        fire(this, new Event('error'))
        break
      case 'close':
        this.#readyState = states.CLOSED
        fire(this, new CloseEvent('close', { code, reason, wasClean }))
        break
    }
  }
}

for (const target of [WebSocket, WebSocket.prototype]) {
  defineValues(target, states, { enumerable: true })
}
defineEventHandlers(WebSocket.prototype, 'open', 'message', 'error', 'close')
setToStringTag(WebSocket)
