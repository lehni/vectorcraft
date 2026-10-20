import { formatError } from './format-error.js'
import { natives } from './natives.js'

// What a browser's global object has besides its classes, for scripts outside
// one: `performance`, `queueMicrotask()`, `reportError()`, `atob()` and
// `btoa()`.

export const performance = {
  timeOrigin: Date.now(),

  // The milliseconds since the runtime started.
  now() {
    return natives.now()
  }
}

// `callback` runs once the script's current job is done; what it throws is
// reported, as a listener's is.
export function queueMicrotask(callback) {
  if (typeof callback !== 'function') {
    throw new TypeError(`Invalid callback: '${callback}', not a function.`)
  }
  Promise.resolve().then(() => {
    try {
      callback()
    } catch (err) {
      reportError(err)
    }
  })
}

// An error as uncaught ones are reported: in the console.
export function reportError(err) {
  console.error(`Uncaught ${formatError(err)}`)
}

const alphabet =
  'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'

// The bytes `data` encodes in base64, each as the character of its code. As
// browsers have it, whitespace and missing padding are fine.
export function atob(data) {
  let text = String(data).replace(/[\t\n\f\r ]/g, '')
  if (text.length % 4 === 0) text = text.replace(/==?$/, '')
  if (text.length % 4 === 1 || /[^A-Za-z0-9+/]/.test(text)) {
    throw new DOMException(
      'Invalid base64: the string is not correctly encoded.',
      'InvalidCharacterError'
    )
  }
  let bits = 0
  let count = 0
  let result = ''
  for (const char of text) {
    bits = (bits << 6) | alphabet.indexOf(char)
    count += 6
    if (count >= 8) {
      count -= 8
      result += String.fromCharCode((bits >> count) & 0xff)
    }
  }
  return result
}

// `data`'s characters as bytes, in base64; characters above 255 throw.
export function btoa(data) {
  const text = String(data)
  if (/[^\0-\xff]/.test(text)) {
    throw new DOMException(
      'Invalid character: btoa() takes characters up to 255 only.',
      'InvalidCharacterError'
    )
  }
  let result = ''
  for (let i = 0; i < text.length; i += 3) {
    const [a, b, c] = [0, 1, 2].map(j => text.charCodeAt(i + j))
    const bits = (a << 16) | ((b || 0) << 8) | (c || 0)
    result += (
      alphabet[(bits >> 18) & 63] +
      alphabet[(bits >> 12) & 63] +
      (i + 1 < text.length ? alphabet[(bits >> 6) & 63] : '=') +
      (i + 2 < text.length ? alphabet[bits & 63] : '=')
    )
  }
  return result
}
