import { defineAccessors, setToStringTag } from './define.js'
import { natives } from './natives.js'

// `URL` and `URLSearchParams` for scripts outside a browser, as the WHATWG URL
// standard has them: the runtime parses, and sets the parts. `url.searchParams`
// and its URL stay in step both ways.

const parts = [
  'origin',
  'protocol',
  'username',
  'password',
  'host',
  'hostname',
  'port',
  'pathname',
  'search',
  'hash'
]

// What `URLSearchParams` and `URL` reach of each other:
let setPairs
let setSearch

export class URL {
  // The parts as the runtime parsed them:
  #parts
  #searchParams = null

  constructor(url, base) {
    this.#parts = parse(url, base)
    if (!this.#parts) {
      throw new TypeError(`Invalid URL: '${url}'.`)
    }
  }

  static {
    setSearch = (url, query) => {
      url.#parts = natives.url.set(url.#parts.href, 'search', query)
    }
  }

  static canParse(url, base) {
    return !!parse(url, base)
  }

  static parse(url, base) {
    return parse(url, base) ? new URL(url, base) : null
  }

  get href() {
    return this.#parts.href
  }

  set href(href) {
    this.#set('href', href)
  }

  get searchParams() {
    if (!this.#searchParams) {
      this.#searchParams = new URLSearchParams(this.#parts.search)
      linksByParams.set(this.#searchParams, this)
    }
    return this.#searchParams
  }

  toString() {
    return this.#parts.href
  }

  toJSON() {
    return this.#parts.href
  }

  // Set a part, and the search params follow.
  #set(part, value) {
    this.#parts = natives.url.set(this.#parts.href, part, String(value))
    if (this.#searchParams) {
      setPairs(this.#searchParams, parseQuery(this.#parts.search))
    }
  }

  static {
    for (const part of parts) {
      const accessor = {
        get() {
          return this.#parts[part]
        }
      }
      // The origin can't be set:
      if (part !== 'origin') {
        accessor.set = function (value) {
          this.#set(part, value)
        }
      }
      defineAccessors(URL.prototype, { [part]: accessor })
    }
  }
}

// The URL that `URLSearchParams` belong to, which they change:
const linksByParams = new WeakMap()

export class URLSearchParams {
  #pairs = []

  // A query string (with or without `?`), `[[name, value], …]`,
  // `{ name: value }` or another `URLSearchParams`.
  constructor(init = '') {
    if (init instanceof URLSearchParams) {
      this.#pairs = init.#pairs.map(pair => [...pair])
    } else if (init !== null && typeof init === 'object') {
      this.#pairs =
        typeof init[Symbol.iterator] === 'function'
          ? Array.from(init, pair => {
              pair = Array.from(pair, String)
              if (pair.length !== 2) {
                throw new TypeError(`Invalid pair: '${pair}'.`)
              }
              return pair
            })
          : Object.entries(init).map(([name, value]) => [name, String(value)])
    } else {
      this.#pairs = parseQuery(String(init))
    }
  }

  static {
    setPairs = (params, pairs) => {
      params.#pairs = pairs
    }
  }

  get size() {
    return this.#pairs.length
  }

  append(name, value) {
    this.#pairs.push([String(name), String(value)])
    this.#update()
  }

  delete(name, value) {
    name = String(name)
    this.#pairs = this.#pairs.filter(
      pair => (
        pair[0] !== name || value !== undefined && pair[1] !== String(value)
      )
    )
    this.#update()
  }

  get(name) {
    return this.#pairs.find(pair => pair[0] === String(name))?.[1] ?? null
  }

  getAll(name) {
    return this.#pairs
      .filter(pair => pair[0] === String(name))
      .map(([, value]) => value)
  }

  has(name, value) {
    return this.#pairs.some(
      pair => (
        pair[0] === String(name) &&
        (value === undefined || pair[1] === String(value))
      )
    )
  }

  // In place of the first one of its name:
  set(name, value) {
    const pair = [String(name), String(value)]
    const index = this.#pairs.findIndex(([name]) => name === pair[0])
    this.#pairs = this.#pairs.filter(([name]) => name !== pair[0])
    this.#pairs.splice(index < 0 ? this.#pairs.length : index, 0, pair)
    this.#update()
  }

  // By name, in UTF-16 code units, as strings compare: stable.
  sort() {
    this.#pairs.sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    this.#update()
  }

  forEach(callback, thisArg) {
    for (const [name, value] of this) {
      callback.call(thisArg, value, name, this)
    }
  }

  *entries() {
    for (const [name, value] of this.#pairs) yield [name, value]
  }

  *keys() {
    for (const [name] of this.#pairs) yield name
  }

  *values() {
    for (const [, value] of this.#pairs) yield value
  }

  [Symbol.iterator]() {
    return this.entries()
  }

  toString() {
    return natives.url.serializeQuery(this.#pairs)
  }

  // The URL they belong to follows.
  #update() {
    const url = linksByParams.get(this)
    if (url) setSearch(url, this.toString())
  }
}

setToStringTag(URL, URLSearchParams)

// The parts of `url`, against `base` when it's given, or `null`.
function parse(url, base) {
  return base === undefined
    ? natives.url.parse(String(url))
    : natives.url.parse(String(url), String(base))
}

function parseQuery(query) {
  return natives.url.parseQuery(query.replace(/^\?/, ''))
}
