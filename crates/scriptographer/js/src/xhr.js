import { Blob } from './blob.js'
import {
  checkArguments,
  checkInternal,
  defineConstants,
  internal,
  setToStringTag
} from './define.js'
import { dom } from './dom.js'
import {
  Event,
  EventTarget,
  ProgressEvent,
  defineEventHandlers,
  fire
} from './events.js'

// `XMLHttpRequest` for scripts outside a browser, on `fetch()`. Requests are
// asynchronous only, as scripts can't wait for the network, and as bodies
// arrive whole, each direction reports its progress once. Text is UTF-8.

const states = {
  UNSENT: 0,
  OPENED: 1,
  HEADERS_RECEIVED: 2,
  LOADING: 3,
  DONE: 4
}
const responseTypes = ['', 'arraybuffer', 'blob', 'document', 'json', 'text']
// The methods it normalizes to upper case, and those it refuses:
const methods = ['DELETE', 'GET', 'HEAD', 'OPTIONS', 'POST', 'PUT']
const forbiddenMethods = ['CONNECT', 'TRACE', 'TRACK']
const token = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/

export class XMLHttpRequestEventTarget extends EventTarget {
  constructor(key) {
    checkInternal(
      key,
      'XMLHttpRequestEventTarget',
      'XMLHttpRequest and its upload are them'
    )
    super()
  }
}

export class XMLHttpRequestUpload extends XMLHttpRequestEventTarget {}

export class XMLHttpRequest extends XMLHttpRequestEventTarget {
  #readyState = states.UNSENT
  #method = 'GET'
  #url = null
  #headers = new Headers()
  #isSent = false
  #isUploading = false
  #timeout = 0
  #withCredentials = false
  #responseType = ''
  #mimeType = null
  #upload = new XMLHttpRequestUpload(internal)
  // The request in flight, which a later one or `abort()` replaces, and how to
  // stop it:
  #request = null
  #controller = null
  #timer = null
  // What came back: a `Response` without its body, the body's bytes, and what
  // `response` made of them.
  #response = null
  #bytes = null
  #value

  constructor() {
    super(internal)
  }

  get readyState() {
    return this.#readyState
  }

  get upload() {
    return this.#upload
  }

  get timeout() {
    return this.#timeout
  }

  set timeout(timeout) {
    this.#timeout = Math.max(0, Math.trunc(Number(timeout)) || 0)
  }

  // There are no cookies outside a browser; it's kept for libraries that ask.
  get withCredentials() {
    return this.#withCredentials
  }

  set withCredentials(withCredentials) {
    if (this.#isSent || this.#readyState > states.OPENED) {
      throw new DOMException(
        'Unable to set withCredentials: the request was sent.',
        'InvalidStateError'
      )
    }
    this.#withCredentials = !!withCredentials
  }

  get responseType() {
    return this.#responseType
  }

  set responseType(type) {
    this.#checkNotLoading('responseType')
    type = String(type)
    if (responseTypes.includes(type)) this.#responseType = type
  }

  open(method, url, async = true, user, password) {
    checkArguments(arguments, 2, 'open')
    method = String(method)
    if (!token.test(method)) {
      throw new DOMException(
        `Invalid XMLHttpRequest method: '${method}'.`,
        'SyntaxError'
      )
    }
    const upper = method.toUpperCase()
    if (forbiddenMethods.includes(upper)) {
      throw new DOMException(
        `Unsupported XMLHttpRequest method: '${method}'.`,
        'SecurityError'
      )
    }
    const base = globalThis.location?.href
    url = String(url)
    if (!URL.canParse(url, base)) {
      throw new DOMException(
        `Invalid XMLHttpRequest URL: '${url}'.`,
        'SyntaxError'
      )
    }
    if (!async) {
      throw new DOMException(
        "Unsupported synchronous XMLHttpRequest: scripts can't wait for " +
        'the network.',
        'InvalidAccessError'
      )
    }
    const parsed = new URL(url, base)
    if (user != null) parsed.username = user
    if (password != null) parsed.password = password
    this.#stop()
    this.#method = methods.includes(upper) ? upper : method
    this.#url = parsed.href
    this.#headers = new Headers()
    this.#isSent = false
    this.#response = this.#bytes = null
    this.#value = undefined
    if (this.#readyState !== states.OPENED) {
      this.#readyState = states.OPENED
      fire(this, new Event('readystatechange'))
    }
  }

  setRequestHeader(name, value) {
    checkArguments(arguments, 2, 'setRequestHeader')
    this.#checkOpened('setRequestHeader')
    name = String(name)
    if (!token.test(name)) {
      throw new DOMException(
        `Invalid XMLHttpRequest header: '${name}'.`,
        'SyntaxError'
      )
    }
    const values = this.#headers.get(name)
    this.#headers.set(name, values === null ? value : `${values}, ${value}`)
  }

  send(body = null) {
    this.#checkOpened('send')
    if (this.#method === 'GET' || this.#method === 'HEAD') body = null
    const request = (this.#request = {})
    const controller = (this.#controller = new AbortController())
    this.#isSent = true
    this.#isUploading = body != null
    fireProgress(this, 'loadstart')
    if (this.#isUploading) fireProgress(this.#upload, 'loadstart')
    if (this.#timeout > 0) {
      this.#timer = setTimeout(() => this.#fail('timeout'), this.#timeout)
    }
    const isCurrent = () => this.#request === request
    fetch(this.#url, {
      method: this.#method,
      headers: this.#headers,
      body,
      signal: controller.signal
    }).then(
      async response => {
        if (isCurrent()) {
          if (this.#isUploading) {
            this.#isUploading = false
            for (const type of ['progress', 'load', 'loadend']) {
              fireProgress(this.#upload, type)
            }
          }
          this.#response = response
          this.#setState(states.HEADERS_RECEIVED)
          const bytes = await response.bytes()
          if (isCurrent()) {
            this.#bytes = bytes
            this.#setState(states.LOADING)
            const { length } = bytes
            fireProgress(this, 'progress', length, length)
            this.#finish()
            this.#setState(states.DONE)
            fireProgress(this, 'load', length, length)
            fireProgress(this, 'loadend', length, length)
          }
        }
      },
      () => {
        if (isCurrent()) this.#fail('error')
      }
    )
  }

  abort() {
    const state = this.#readyState
    if (
      state === states.OPENED && this.#isSent ||
      state === states.HEADERS_RECEIVED ||
      state === states.LOADING
    ) {
      this.#fail('abort')
    } else {
      this.#stop()
    }
    if (this.#readyState === states.DONE) {
      // Back to the start, without an event:
      this.#readyState = states.UNSENT
      this.#response = this.#bytes = null
    }
  }

  get status() {
    return this.#response?.status ?? 0
  }

  get statusText() {
    return this.#response?.statusText ?? ''
  }

  get responseURL() {
    return this.#response?.url ?? ''
  }

  getResponseHeader(name) {
    checkArguments(arguments, 1, 'getResponseHeader')
    return this.#response?.headers.get(name) ?? null
  }

  // Sorted by name, in lower case, the values of a name joined:
  getAllResponseHeaders() {
    return [...(this.#response?.headers ?? [])]
      .map(([name, value]) => `${name}: ${value}\r\n`)
      .join('')
  }

  overrideMimeType(mime) {
    checkArguments(arguments, 1, 'overrideMimeType')
    this.#checkNotLoading('overrideMimeType')
    this.#mimeType = String(mime)
  }

  get responseText() {
    if (this.#responseType !== '' && this.#responseType !== 'text') {
      throw new DOMException(
        `Unable to read responseText of a '${this.#responseType}' response.`,
        'InvalidStateError'
      )
    }
    return this.#bytes ? new TextDecoder().decode(this.#bytes) : ''
  }

  get response() {
    const type = this.#responseType
    return type === '' || type === 'text'
      ? this.responseText
      : this.#readyState !== states.DONE
        ? null
        : this.#value === undefined
          ? (this.#value = this.#getResponse(type))
          : this.#value
  }

  get responseXML() {
    const type = this.#responseType
    if (type !== '' && type !== 'document') {
      throw new DOMException(
        `Unable to read responseXML of a '${type}' response.`,
        'InvalidStateError'
      )
    }
    return this.#readyState !== states.DONE
      ? null
      : this.#value === undefined
        ? (this.#value = this.#getDocument())
        : this.#value
  }

  #getResponse(type) {
    const bytes = this.#bytes ?? new Uint8Array()
    switch (type) {
      case 'json':
        try {
          return JSON.parse(new TextDecoder().decode(bytes))
        } catch {
          return null
        }
      case 'arraybuffer':
        return bytes.slice().buffer
      case 'blob':
        return new Blob([bytes], { type: this.#getMimeType() })
      case 'document':
        return this.#getDocument()
    }
  }

  // XML as a document, and HTML when `responseType` asks for a document:
  #getDocument() {
    const mime = this.#getMimeType().split(';')[0].trim().toLowerCase()
    const kind =
      mime === 'text/html'
        ? this.#responseType === 'document' && 'text/html'
        : (
            mime === 'text/xml' ||
            mime === 'application/xml' ||
            mime.endsWith('+xml')
          ) &&
          'text/xml'
    if (kind && this.#bytes) {
      try {
        const parser = new dom.window.DOMParser()
        return parser.parseFromString(this.responseText, kind)
      } catch {
        return null
      }
    }
    return null
  }

  // The overridden type, else the response's, else XML, as the standard has it:
  #getMimeType() {
    return (
      this.#mimeType ??
      this.#response?.headers.get('content-type') ??
      'text/xml'
    )
  }

  #setState(state) {
    this.#readyState = state
    fire(this, new Event('readystatechange'))
  }

  #checkOpened(name) {
    if (this.#readyState !== states.OPENED || this.#isSent) {
      throw new DOMException(
        `Unable to ${name}(): the request isn't opened, or was sent.`,
        'InvalidStateError'
      )
    }
  }

  #checkNotLoading(name) {
    if (this.#readyState >= states.LOADING) {
      throw new DOMException(
        `Unable to set ${name}: the response is loading or loaded.`,
        'InvalidStateError'
      )
    }
  }

  // The request done, its timeout cleared:
  #finish() {
    clearTimeout(this.#timer)
    this.#request = this.#controller = this.#timer = null
    this.#isSent = false
  }

  // The request in flight stopped, without an event:
  #stop() {
    this.#controller?.abort()
    this.#finish()
  }

  // The request failed with `type`, 'error', 'abort' or 'timeout', as a network
  // error: no response, and events to say so.
  #fail(type) {
    const isUploading = this.#isUploading
    this.#isUploading = false
    this.#stop()
    this.#response = this.#bytes = null
    this.#setState(states.DONE)
    if (isUploading) {
      fireProgress(this.#upload, type)
      fireProgress(this.#upload, 'loadend')
    }
    fireProgress(this, type)
    fireProgress(this, 'loadend')
  }
}

defineConstants(XMLHttpRequest, states)
defineEventHandlers(
  XMLHttpRequestEventTarget.prototype,
  'loadstart',
  'progress',
  'abort',
  'error',
  'load',
  'timeout',
  'loadend'
)
defineEventHandlers(XMLHttpRequest.prototype, 'readystatechange')
setToStringTag(XMLHttpRequestEventTarget, XMLHttpRequestUpload, XMLHttpRequest)

function fireProgress(target, type, loaded = 0, total = 0) {
  fire(
    target,
    new ProgressEvent(type, { lengthComputable: total > 0, loaded, total })
  )
}
