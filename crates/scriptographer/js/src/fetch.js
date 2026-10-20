import { Blob, File, getBufferBytes, getBytes } from './blob.js'
import { checkArguments, inject, setToStringTag } from './define.js'
import { natives } from './natives.js'
import { getPairs, pairIterable, setFirst } from './pairs.js'

// The browser's `fetch()` for scripts outside a browser, with `Headers`,
// `FormData`, `Request` and `Response`. The runtime sends the requests, on
// threads of its own. Bodies arrive whole: no streams. There is no CORS, cookie
// store or HTTP cache outside a browser.

export class Headers {
  // The headers as they came, names in lower case:
  #list = []

  constructor(init) {
    if (init instanceof Headers) {
      this.#list = [...init.#list]
    } else if (init != null) {
      for (const [name, value] of getPairs(init)) this.append(name, value)
    }
  }

  get(name) {
    const values = this.#getValues(name)
    return values.length ? values.join(', ') : null
  }

  getSetCookie() {
    return this.#getValues('set-cookie')
  }

  has(name) {
    return this.#getValues(name).length > 0
  }

  set(name, value) {
    this.#list = setFirst(this.#list, getHeader(name, value))
  }

  append(name, value) {
    this.#list.push(getHeader(name, value))
  }

  delete(name) {
    name = getName(name)
    this.#list = this.#list.filter(header => header[0] !== name)
  }

  // As scripts see them: sorted by name, the values of a name joined.
  *entries() {
    const names = [...new Set(this.#list.map(([name]) => name))].sort()
    for (const name of names) yield [name, this.get(name)]
  }

  #getValues(name) {
    name = getName(name)
    return this.#list
      .filter(header => header[0] === name)
      .map(([, value]) => value)
  }
}

export class FormData {
  // `[name, value]`, a value a string or a `File`:
  #entries = []

  append(name, value, filename) {
    checkArguments(arguments, 2, 'append')
    this.#entries.push(getEntry(...arguments))
  }

  set(name, value, filename) {
    checkArguments(arguments, 2, 'set')
    this.#entries = setFirst(this.#entries, getEntry(...arguments))
  }

  get(name) {
    return this.#entries.find(entry => entry[0] === String(name))?.[1] ?? null
  }

  getAll(name) {
    return this.#entries
      .filter(entry => entry[0] === String(name))
      .map(([, value]) => value)
  }

  has(name) {
    return this.#entries.some(entry => entry[0] === String(name))
  }

  delete(name) {
    this.#entries = this.#entries.filter(entry => entry[0] !== String(name))
  }

  *entries() {
    for (const [name, value] of this.#entries) yield [name, value]
  }
}

for (const Class of [Headers, FormData]) {
  inject(Class.prototype, pairIterable)
}

// What `Request` and `Response` share: their body, read once.
class Body {
  #bytes
  #isUsed = false

  // The body's bytes, its type in `headers` unless they have one.
  constructor(body, headers) {
    if (body != null) {
      const { bytes, type } = getBody(body)
      if (type && !headers.has('content-type')) {
        headers.set('content-type', type)
      }
      this.#bytes = bytes
    }
  }

  get bodyUsed() {
    return this.#isUsed
  }

  async text() {
    return new TextDecoder().decode(this.#take())
  }

  async json() {
    return JSON.parse(await this.text())
  }

  async arrayBuffer() {
    return this.#take().slice().buffer
  }

  async bytes() {
    return this.#take().slice()
  }

  async blob() {
    const type = this.headers.get('content-type') ?? ''
    return new Blob([this.#take()], { type })
  }

  #take() {
    if (this.#isUsed) {
      throw new TypeError('Unable to read the body: it was read already.')
    }
    this.#isUsed = true
    return this.#bytes ?? new Uint8Array()
  }

  // The body, unread, for a copy.
  static getBytes(body) {
    if (body.#isUsed) {
      throw new TypeError('Unable to copy the body: it was read already.')
    }
    return body.#bytes ?? null
  }
}

export class Request extends Body {
  #url
  #method
  #headers
  #signal
  #redirect

  constructor(input, init = {}) {
    const from = input instanceof Request ? input : null
    const method = String(init.method ?? from?.method ?? 'GET').toUpperCase()
    const headers = new Headers(init.headers ?? from?.headers)
    const body = init.body ?? (from && Body.getBytes(from))
    if (body != null && (method === 'GET' || method === 'HEAD')) {
      throw new TypeError(`Invalid Request: a ${method} request has no body.`)
    }
    super(body, headers)
    this.#url = from ? from.url : String(input)
    this.#method = method
    this.#headers = headers
    this.#signal = init.signal ?? from?.signal ?? null
    this.#redirect = String(init.redirect ?? from?.redirect ?? 'follow')
  }

  get url() {
    return this.#url
  }

  get method() {
    return this.#method
  }

  get headers() {
    return this.#headers
  }

  get signal() {
    return this.#signal
  }

  get redirect() {
    return this.#redirect
  }

  clone() {
    return new Request(this)
  }
}

let setResponse

export class Response extends Body {
  #status
  #statusText
  #headers
  #url = ''
  #redirected = false
  #type = 'default'

  constructor(body = null, { status = 200, statusText = '', headers } = {}) {
    status = Number(status)
    if (!(status >= 200 && status <= 599)) {
      throw new RangeError(`Invalid Response status: '${status}'.`)
    }
    headers = new Headers(headers)
    super(body, headers)
    this.#status = status
    this.#statusText = String(statusText)
    this.#headers = headers
  }

  static {
    // What a response that came over the network says of itself:
    setResponse = (response, { url, redirected, type }) => {
      response.#url = url
      response.#redirected = redirected
      response.#type = type
      return response
    }
  }

  get status() {
    return this.#status
  }

  get statusText() {
    return this.#statusText
  }

  get ok() {
    return this.#status >= 200 && this.#status <= 299
  }

  get headers() {
    return this.#headers
  }

  get url() {
    return this.#url
  }

  get redirected() {
    return this.#redirected
  }

  get type() {
    return this.#type
  }

  clone() {
    const { status, statusText, headers } = this
    return setResponse(
      new Response(Body.getBytes(this), { status, statusText, headers }),
      this
    )
  }
}

setToStringTag(Headers, FormData, Request, Response)

// A Promise of the `Response`, rejected with a `TypeError` when the request
// fails, or with the signal's reason when it aborts.
export async function fetch(input, init) {
  const request = new Request(input, init)
  const { method, url, headers, signal, redirect } = request
  signal?.throwIfAborted()
  const body = Body.getBytes(request)
  return new Promise((resolve, reject) => {
    const abort = () => reject(signal.reason)
    signal?.addEventListener('abort', abort)
    natives
      .request(method, url, [...headers], body, redirect === 'follow')
      .then(({ status, statusText, url, redirected, headers, body }) => {
        const response = new Response(body, { status, statusText, headers })
        resolve(setResponse(response, { url, redirected, type: 'basic' }))
      }, reject)
      .finally(() => signal?.removeEventListener('abort', abort))
  })
}

// A body's bytes and the content type it implies, which the runtime reads too:
// a string, a buffer or a view, a `Blob`, `FormData` or `URLSearchParams`.
export function getBody(body) {
  const bytes = getBufferBytes(body)
  return bytes
    ? { bytes, type: null }
    : body instanceof Blob
      ? { bytes: getBytes(body), type: body.type || null }
      : body instanceof FormData
        ? getMultipart(body)
        : {
            bytes: new TextEncoder().encode(String(body)),
            type:
              body instanceof URLSearchParams
                ? 'application/x-www-form-urlencoded;charset=UTF-8'
                : 'text/plain;charset=UTF-8'
          }
}

// A header name as headers compare them, lower case, and its value, trimmed:
function getHeader(name, value) {
  return [getName(name), String(value).replace(/^[\t\n\r ]+|[\t\n\r ]+$/g, '')]
}

function getName(name) {
  return String(name).trim().toLowerCase()
}

// A `FormData` entry from `append(name, value[, filename])`: a `Blob` becomes
// a `File`, named `filename` or 'blob' if it has no name.
function getEntry(name, value, filename) {
  return [
    String(name),
    value instanceof Blob
      ? value instanceof File && filename === undefined
        ? value
        : new File([value], filename ?? value.name ?? 'blob', {
            type: value.type
          })
      : String(value)
  ]
}

// `FormData` as `multipart/form-data`, with a random boundary.
function getMultipart(form) {
  const boundary = `----Scriptographer${
    Array.from({ length: 24 }, () =>
      Math.floor(Math.random() * 16).toString(16)
    ).join('')
  }`
  const quote = text =>
    text.replace(/\r/g, '%0D').replace(/\n/g, '%0A').replace(/"/g, '%22')
  const parts = []
  for (const [name, value] of form) {
    const disposition = `--${
      boundary
    }\r\nContent-Disposition: form-data; name="${
      quote(name)
    }"`
    if (value instanceof File) {
      const type = value.type || 'application/octet-stream'
      parts.push(
        `${disposition}; filename="${quote(value.name)}"\r\nContent-Type: ${
          type
        }\r\n\r\n`,
        value,
        '\r\n'
      )
    } else {
      parts.push(`${disposition}\r\n\r\n${value}\r\n`)
    }
  }
  parts.push(`--${boundary}--\r\n`)
  return {
    bytes: getBytes(new Blob(parts)),
    type: `multipart/form-data; boundary=${boundary}`
  }
}
