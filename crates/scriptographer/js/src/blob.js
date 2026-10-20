import { natives } from './natives.js'
import { setToStringTag } from './define.js'

// `Blob` and `File` for scripts outside a browser. Their bytes arrive whole: no
// streams.

// A blob's bytes, out of its own properties: a `LocalFile` is a `File` the
// runtime makes, whose bytes it reads where the file is.
const bytesByBlob = new WeakMap()

export class Blob {
  constructor(parts = [], { type = '' } = {}) {
    bytesByBlob.set(this, { bytes: getPartBytes(parts), type: getType(type) })
  }

  get size() {
    return getBytes(this).length
  }

  get type() {
    return bytesByBlob.get(this).type
  }

  slice(start = 0, end, type = '') {
    const bytes = getBytes(this)
    return new Blob([bytes.slice(start, end ?? bytes.length)], { type })
  }

  async text() {
    return new TextDecoder().decode(getBytes(this))
  }

  async arrayBuffer() {
    return getBytes(this).slice().buffer
  }

  async bytes() {
    return getBytes(this).slice()
  }
}

export class File extends Blob {
  #name
  #lastModified

  constructor(parts, name, options = {}) {
    if (arguments.length < 2) {
      throw new TypeError('Invalid File: its parts and its name are needed.')
    }
    super(parts, options)
    this.#name = String(name)
    this.#lastModified = Math.trunc(Number(options.lastModified ?? Date.now()))
  }

  get name() {
    return this.#name
  }

  get lastModified() {
    return this.#lastModified
  }

  get webkitRelativePath() {
    return ''
  }
}

setToStringTag(Blob, File)

// The bytes of `blob`, which the runtime reads too, not copied.
export function getBytes(blob) {
  return bytesByBlob.get(blob)?.bytes ?? natives.readFile(blob)
}

// The bytes of a `Blob`'s parts: strings (UTF-8), buffers, views and `Blob`s.
function getPartBytes(parts) {
  const encoder = new TextEncoder()
  const list = Array.from(parts, part =>
    part instanceof Blob
      ? getBytes(part)
      : part instanceof ArrayBuffer
        ? new Uint8Array(part)
        : ArrayBuffer.isView(part)
          ? new Uint8Array(part.buffer, part.byteOffset, part.byteLength)
          : encoder.encode(String(part))
  )
  const bytes = new Uint8Array(
    list.reduce((size, part) => size + part.length, 0)
  )
  let offset = 0
  for (const part of list) {
    bytes.set(part, offset)
    offset += part.length
  }
  return bytes
}

// A MIME type as blobs keep it: lower case, empty unless it's printable ASCII.
function getType(type) {
  type = String(type)
  return /^[\x20-\x7e]*$/.test(type) ? type.toLowerCase() : ''
}
