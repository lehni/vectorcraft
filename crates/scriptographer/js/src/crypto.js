import { setToStringTag } from './define.js'
import { natives } from './natives.js'

// `crypto.getRandomValues()` and `crypto.randomUUID()` for scripts outside a
// browser, from the runtime's random bytes.

const integerArrays = [
  Int8Array,
  Uint8Array,
  Uint8ClampedArray,
  Int16Array,
  Uint16Array,
  Int32Array,
  Uint32Array,
  BigInt64Array,
  BigUint64Array
]

export class Crypto {
  // At most 65536 bytes, as in browsers.
  getRandomValues(array) {
    if (!integerArrays.some(Class => array instanceof Class)) {
      throw new DOMException(
        `Invalid array: '${array}', not an integer typed array.`,
        'TypeMismatchError'
      )
    }
    if (array.byteLength > 65536) {
      throw new DOMException(
        'Invalid array: more than 65536 bytes.',
        'QuotaExceededError'
      )
    }
    new Uint8Array(array.buffer, array.byteOffset, array.byteLength).set(
      natives.random(array.byteLength)
    )
    return array
  }

  // A random version 4 UUID.
  randomUUID() {
    const bytes = natives.random(16)
    // Version 4, variant 10xx:
    bytes[6] = (bytes[6] & 0x0f) | 0x40
    bytes[8] = (bytes[8] & 0x3f) | 0x80
    const hex = Array.from(bytes, byte =>
      byte.toString(16).padStart(2, '0')
    ).join('')
    return [
      hex.slice(0, 8),
      hex.slice(8, 12),
      hex.slice(12, 16),
      hex.slice(16, 20),
      hex.slice(20)
    ].join('-')
  }
}

setToStringTag(Crypto)
