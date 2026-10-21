import { getBufferBytes } from './blob.js'
import {
  checkArguments,
  checkInternal,
  inject,
  internal,
  setToStringTag
} from './define.js'
import { EventTarget } from './events.js'
import { natives } from './natives.js'

// The CSS Font Loading API for scripts outside a browser: a `FontFace` loads a
// font from a URL or from bytes into VectorCraft's fonts, and once the script
// adds it to `fonts`, its canvases know it by the family the script gave it.

// The descriptors of a `FontFace`, with their defaults:
const defaults = {
  style: 'normal',
  weight: 'normal',
  stretch: 'normal',
  unicodeRange: 'U+0-10FFFF',
  featureSettings: 'normal',
  variationSettings: 'normal',
  display: 'auto',
  ascentOverride: 'normal',
  descentOverride: 'normal',
  lineGapOverride: 'normal'
}

let getFamilies

export class FontFace {
  #family
  #source
  #status = 'unloaded'
  // The font's families, as VectorCraft knows them, once it's loaded:
  #families = null
  #loaded
  #settle

  constructor(family, source, descriptors = {}) {
    checkArguments(arguments, 2, 'FontFace')
    this.#family = String(family)
    this.#source = source
    this.#loaded = new Promise((resolve, reject) => {
      this.#settle = { resolve, reject }
    })
    // A face that fails without anyone waiting for it is no unhandled error:
    this.#loaded.catch(() => {})
    for (const key of Object.keys(defaults)) {
      if (key in descriptors) {
        this[key] = descriptors[key]
      }
    }
    // A face of bytes loads at once, as browsers load them:
    if (typeof source !== 'string') {
      this.load()
    }
  }

  static {
    getFamilies = face => face.#families
  }

  get family() {
    return this.#family
  }

  set family(family) {
    this.#family = String(family)
  }

  get status() {
    return this.#status
  }

  get loaded() {
    return this.#loaded
  }

  load() {
    if (this.#status === 'unloaded') {
      this.#status = 'loading'
      getFontBytes(this.#source)
        .then(bytes => {
          const families = natives.canvas.addFont(bytes)
          if (!families.length) {
            throw new DOMException(
              `Invalid font: '${this.#family}' is no font file.`,
              'SyntaxError'
            )
          }
          this.#families = families
          this.#status = 'loaded'
          this.#settle.resolve(this)
        })
        .catch(err => {
          this.#status = 'error'
          this.#settle.reject(
            err instanceof DOMException
              ? err
              : new DOMException(
                  `Unable to load font '${this.#family}': ${err.message}`,
                  'NetworkError'
                )
          )
        })
    }
    return this.#loaded
  }
}

// The descriptors as accessors, kept as text:
for (const key of Object.keys(defaults)) {
  const values = new WeakMap()
  inject(
    FontFace.prototype,
    {
      get [key]() {
        return values.get(this) ?? defaults[key]
      },

      set [key](value) {
        values.set(this, String(value))
      }
    },
    { enumerable: true }
  )
}

// The fonts of the script, `fonts`, as a worker's and a document's: its faces'
// families are those of its canvases.
export class FontFaceSet extends EventTarget {
  #faces = new Set()

  constructor(key) {
    checkInternal(key, 'FontFaceSet', 'scripts have one, fonts')
    super()
  }

  get size() {
    return this.#faces.size
  }

  get status() {
    return [...this.#faces].some(face => face.status === 'loading')
      ? 'loading'
      : 'loaded'
  }

  // Once the faces that are loading now loaded or failed:
  get ready() {
    const loading = [...this.#faces].filter(face => face.status === 'loading')
    return Promise.allSettled(loading.map(face => face.loaded)).then(
      () => this
    )
  }

  add(face) {
    checkFace(face)
    if (!this.#faces.has(face)) {
      this.#faces.add(face)
      face.loaded.then(
        () => {
          if (this.#faces.has(face)) {
            show(face, true)
          }
        },
        () => {}
      )
    }
    return this
  }

  delete(face) {
    checkFace(face)
    const deleted = this.#faces.delete(face)
    if (deleted && face.status === 'loaded') {
      show(face, false)
    }
    return deleted
  }

  clear() {
    for (const face of [...this.#faces]) {
      this.delete(face)
    }
  }

  has(face) {
    return this.#faces.has(face)
  }

  forEach(callback, thisArg) {
    for (const face of this.#faces) {
      callback.call(thisArg, face, face, this)
    }
  }

  values() {
    return this.#faces.values()
  }

  keys() {
    return this.#faces.values()
  }

  entries() {
    return this.#faces.entries()
  }

  [Symbol.iterator]() {
    return this.#faces.values()
  }

  // The faces of the families in the CSS `font`, loaded:
  load(font, text = '') {
    return Promise.all(this.#matching(font).map(face => face.load()))
  }

  // Whether the faces of the families in the CSS `font` are all loaded:
  check(font, text = '') {
    return this.#matching(font).every(face => face.status === 'loaded')
  }

  #matching(font) {
    const families = String(font)
      .split(',')
      .map(family =>
        family
          .trim()
          .split(/\s+/)
          .pop()
          .replace(/^['"]|['"]$/g, '')
      )
      .map(family => family.toLowerCase())
    return [...this.#faces].filter(face =>
      families.includes(face.family.toLowerCase())
    )
  }
}

setToStringTag(FontFace, FontFaceSet)

function checkFace(face) {
  if (!(face instanceof FontFace)) {
    throw new TypeError(`Invalid font face: '${face}', not a FontFace.`)
  }
}

// The canvases know the font of `face` by its family while it's `shown`:
function show(face, shown) {
  natives.canvas.setFontFace(face.family, getFamilies(face), shown)
}

// The bytes of a face's source: a `url()` of CSS's `src`, which is fetched, or
// a buffer or view.
async function getFontBytes(source) {
  if (typeof source === 'string') {
    const url = /url\(\s*(['"]?)(.*?)\1\s*\)/.exec(source)?.[2]
    if (!url) {
      throw new DOMException(
        `Invalid font source: '${source}'.`,
        'SyntaxError'
      )
    }
    const response = await fetch(url)
    if (!response.ok) {
      throw new Error(`${response.status} ${response.statusText}`)
    }
    return new Uint8Array(await response.arrayBuffer())
  } else {
    const bytes = getBufferBytes(source)
    if (!bytes) {
      throw new TypeError(`Invalid font source: '${source}'.`)
    }
    return bytes
  }
}
