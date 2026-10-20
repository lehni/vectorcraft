import {
  checkArguments,
  checkInternal,
  inject,
  internal,
  setToStringTag
} from './define.js'
import { natives } from './natives.js'

// `OffscreenCanvas` and its kin for scripts outside a browser, with pixels: its
// `OffscreenCanvasRenderingContext2D` draws as the browser's does. The runtime
// keeps the pixels, paths, gradients, patterns and drawing state in slots, and
// draws (`natives.canvas`); each object here keeps the number of its slot, and
// frees it when the script drops the object.

const { canvas } = natives

// The slots of the objects, which the classes' static blocks reach:
let getCanvasSurface,
  getBitmapSurface,
  getPath,
  getGradientPaint,
  getPatternPaint

// What frees the slots of the objects the script dropped, when the realm can:
const registry = (
  globalThis.FinalizationRegistry &&
  new FinalizationRegistry(([kind, slot]) => canvas.release(kind, slot))
)

// The contexts `getContext()` knows, of which only '2d' is made here:
const contextTypes = ['2d', 'bitmaprenderer', 'webgl', 'webgl2', 'webgpu']

export class OffscreenCanvas {
  #surface
  #width
  #height
  #context = null

  // Transparent black, with a context's defaults. A canvas larger than its
  // pixels may be keeps its size, and has none, as a browser's that couldn't
  // be made.
  constructor(width, height) {
    checkArguments(arguments, 2, 'OffscreenCanvas')
    this.#width = getSize(width)
    this.#height = getSize(height)
    this.#surface = canvas.create(this.#width, this.#height)
    registry?.register(this, ['surface', this.#surface])
  }

  static {
    getCanvasSurface = canvas => canvas.#surface
  }

  // The same 2D context each time, `null` for the other kinds.
  getContext(type, options) {
    checkArguments(arguments, 1, 'getContext')
    type = String(type)
    if (!contextTypes.includes(type)) {
      throw new TypeError(`Invalid context type: '${type}'.`)
    }
    return type === '2d'
      ? (this.#context ??= new OffscreenCanvasRenderingContext2D(
          internal,
          this,
          this.#surface
        ))
      : null
  }

  get width() {
    return this.#width
  }

  // Cleared to the new size, its state reset, as in browsers:
  set width(width) {
    this.#width = getSize(width)
    canvas.resize.call(this.#surface, this.#width, this.#height)
  }

  get height() {
    return this.#height
  }

  set height(height) {
    this.#height = getSize(height)
    canvas.resize.call(this.#surface, this.#width, this.#height)
  }

  async convertToBlob() {
    const png = canvas.toPNG.call(this.#surface)
    return new globalThis.Blob([png], { type: 'image/png' })
  }
}

let getContextSurface

export class OffscreenCanvasRenderingContext2D {
  #canvas
  #surface

  constructor(key, offscreen, surface) {
    checkInternal(
      key,
      'OffscreenCanvasRenderingContext2D',
      'getContext() makes them'
    )
    this.#canvas = offscreen
    this.#surface = surface
  }

  static {
    getContextSurface = context => context.#surface
  }

  get canvas() {
    return this.#canvas
  }

  // Transparent black, of a size or of another `ImageData`'s.
  createImageData(widthOrImage, height) {
    checkArguments(arguments, 1, 'createImageData')
    if (widthOrImage instanceof ImageData) {
      return new ImageData(widthOrImage.width, widthOrImage.height)
    } else {
      checkArguments(arguments, 2, 'createImageData')
      return new ImageData(widthOrImage, height)
    }
  }

  createLinearGradient(x0, y0, x1, y1) {
    checkArguments(arguments, 4, 'createLinearGradient')
    return createGradient('linear', x0, y0, x1, y1)
  }

  createRadialGradient(x0, y0, r0, x1, y1, r1) {
    checkArguments(arguments, 6, 'createRadialGradient')
    return createGradient('radial', x0, y0, r0, x1, y1, r1)
  }

  createConicGradient(angle, x, y) {
    checkArguments(arguments, 3, 'createConicGradient')
    return createGradient('conic', angle, x, y)
  }

  createPattern(image, repetition) {
    checkArguments(arguments, 2, 'createPattern')
    return new CanvasPattern(
      internal,
      canvas.createPattern(getSurface(image), repetition)
    )
  }

  drawImage(image, ...args) {
    checkOverloads(arguments, [3, 5, 9], 'drawImage')
    canvas.methods.drawImage.call(this.#surface, getSurface(image), ...args)
  }

  // The path first, `null` for the current one:
  fill(...args) {
    return callWithPath(this.#surface, 'fill', args, 0)
  }

  stroke(...args) {
    return callWithPath(this.#surface, 'stroke', args, 0)
  }

  clip(...args) {
    return callWithPath(this.#surface, 'clip', args, 0)
  }

  isPointInPath(...args) {
    return callWithPath(this.#surface, 'isPointInPath', args, 2)
  }

  isPointInStroke(...args) {
    return callWithPath(this.#surface, 'isPointInStroke', args, 2)
  }
}

// What the context does with numbers and text alone, and how many arguments
// each needs, or the counts of its overloads:
const contextMethods = {
  getImageData: 4,
  putImageData: [3, 7],
  measureText: 1,
  beginPath: 0,
  closePath: 0,
  moveTo: 2,
  lineTo: 2,
  bezierCurveTo: 6,
  quadraticCurveTo: 4,
  arc: 5,
  arcTo: 5,
  ellipse: 7,
  rect: 4,
  roundRect: 4,
  fillRect: 4,
  strokeRect: 4,
  clearRect: 4,
  fillText: 3,
  strokeText: 3,
  translate: 2,
  rotate: 1,
  scale: 2,
  transform: 6,
  setTransform: [0, 1, 6],
  resetTransform: 0,
  getTransform: 0,
  save: 0,
  restore: 0,
  reset: 0,
  setLineDash: 1,
  getLineDash: 0
}

for (const [name, counts] of Object.entries(contextMethods)) {
  inject(OffscreenCanvasRenderingContext2D.prototype, {
    [name](...args) {
      checkOverloads(args, counts, name)
      return canvas.methods[name].call(getContextSurface(this), ...args)
    }
  })
}

// Its properties, with the slot of a gradient or a pattern given as a style:
for (const name of canvas.properties) {
  inject(
    OffscreenCanvasRenderingContext2D.prototype,
    {
      get [name]() {
        return canvas.get.call(getContextSurface(this), name)
      },

      set [name](value) {
        canvas.set.call(getContextSurface(this), name, value, getPaint(value))
      }
    },
    { enumerable: true }
  )
}

export class ImageData {
  #data
  #width
  #height

  // `new ImageData(width, height)`, transparent black, or
  // `new ImageData(data, width[, height])` of RGBA `data`, not premultiplied.
  constructor(dataOrWidth, width, height) {
    checkArguments(arguments, 2, 'ImageData')
    if (dataOrWidth instanceof Uint8ClampedArray) {
      const data = dataOrWidth
      width = getSide(width)
      height = height === undefined ? data.length / 4 / width : getSide(height)
      if (data.length !== width * height * 4) {
        throw new DOMException(
          'Invalid ImageData: the data is not width × height × 4 bytes.',
          'IndexSizeError'
        )
      }
      this.#data = data
    } else {
      // `(width, height)`:
      height = getSide(width)
      width = getSide(dataOrWidth)
      this.#data = new Uint8ClampedArray(width * height * 4)
    }
    this.#width = width
    this.#height = height
  }

  get width() {
    return this.#width
  }

  get height() {
    return this.#height
  }

  get data() {
    return this.#data
  }

  get colorSpace() {
    return 'srgb'
  }
}

// A path to fill, stroke, clip with or test points against, in its own
// coordinates, from another `Path2D` or SVG path data.
export class Path2D {
  #path

  constructor(path) {
    this.#path = canvas.createPath(
      path instanceof Path2D
        ? path.#path
        : path === undefined
          ? null
          : String(path)
    )
    registry?.register(this, ['path', this.#path])
  }

  static {
    getPath = path => (path instanceof Path2D ? path.#path : undefined)
  }

  addPath(path, transform) {
    checkArguments(arguments, 1, 'addPath')
    canvas.pathMethods.addPath.call(this.#path, getPath(path), transform)
  }
}

for (const name of [
  'closePath',
  'moveTo',
  'lineTo',
  'bezierCurveTo',
  'quadraticCurveTo',
  'arc',
  'arcTo',
  'ellipse',
  'rect',
  'roundRect'
]) {
  const counts = contextMethods[name]
  inject(Path2D.prototype, {
    [name](...args) {
      checkOverloads(args, counts, name)
      return canvas.pathMethods[name].call(getPath(this), ...args)
    }
  })
}

// Decoded images, or copies of an `ImageData`, a canvas or another bitmap, as
// `createImageBitmap()` makes them, to draw with `drawImage()` and
// `createPattern()`; `close()` frees its pixels.
export class ImageBitmap {
  #surface

  constructor(key, surface) {
    checkInternal(key, 'ImageBitmap', 'createImageBitmap() makes them')
    this.#surface = surface
    registry?.register(this, ['surface', surface], this)
  }

  static {
    // Drawing a closed bitmap throws, as in browsers.
    getBitmapSurface = bitmap => {
      if (bitmap.#surface === null) {
        throw new DOMException(
          'the image bitmap is closed',
          'InvalidStateError'
        )
      }
      return bitmap.#surface
    }
  }

  // None once it's closed:
  get width() {
    return this.#surface === null ? 0 : canvas.width.call(this.#surface)
  }

  get height() {
    return this.#surface === null ? 0 : canvas.height.call(this.#surface)
  }

  close() {
    if (this.#surface !== null) {
      registry?.unregister(this)
      canvas.release('surface', this.#surface)
      this.#surface = null
    }
  }
}

export class CanvasGradient {
  #paint

  constructor(key, paint) {
    checkInternal(key, 'CanvasGradient', 'the context makes them')
    this.#paint = paint
    registry?.register(this, ['paint', paint])
  }

  static {
    getGradientPaint = gradient => gradient.#paint
  }

  addColorStop(offset, color) {
    checkArguments(arguments, 2, 'addColorStop')
    canvas.addColorStop.call(this.#paint, offset, color)
  }
}

export class CanvasPattern {
  #paint

  constructor(key, paint) {
    checkInternal(key, 'CanvasPattern', 'the context makes them')
    this.#paint = paint
    registry?.register(this, ['paint', paint])
  }

  static {
    getPatternPaint = pattern => pattern.#paint
  }
}

setToStringTag(
  OffscreenCanvas,
  OffscreenCanvasRenderingContext2D,
  ImageData,
  Path2D,
  ImageBitmap,
  CanvasGradient,
  CanvasPattern
)

// A Promise of an `ImageBitmap` of `image` (a `Blob`, an `ImageData`, a canvas
// or an `ImageBitmap`), cropped, flipped and resized as asked.
export async function createImageBitmap(image, ...args) {
  const source =
    image instanceof globalThis.Blob
      ? new Uint8Array(await image.arrayBuffer())
      : image instanceof ImageData
        ? image
        : getSurface(image)
  return new ImageBitmap(internal, canvas.createImageBitmap(source, ...args))
}

// The surface of a canvas or a bitmap.
function getSurface(image) {
  if (image instanceof OffscreenCanvas) {
    return getCanvasSurface(image)
  } else if (image instanceof ImageBitmap) {
    return getBitmapSurface(image)
  } else {
    throw new TypeError(`Invalid image: '${image}', not a canvas or bitmap.`)
  }
}

// The slot of a gradient or a pattern given as a style, or nothing.
function getPaint(style) {
  return style instanceof CanvasGradient
    ? getGradientPaint(style)
    : style instanceof CanvasPattern
      ? getPatternPaint(style)
      : undefined
}

function createGradient(kind, ...args) {
  return new CanvasGradient(internal, canvas.createGradient(kind, ...args))
}

// `name(path, …)` of the context, with the path's slot first: `null` without
// one, for the current path; `count` arguments after the path are needed.
function callWithPath(surface, name, args, count) {
  const path = args[0] instanceof Path2D ? getPath(args.shift()) : null
  checkArguments(args, count, name)
  return canvas.methods[name].call(surface, path, ...args)
}

// A `TypeError` unless `name()` got as many arguments as it needs, or, for an
// overloaded one, as one of its overloads takes, WebIDL's way: with more than
// the most, the most.
function checkOverloads(args, counts, name) {
  if (typeof counts === 'number') {
    checkArguments(args, counts, name)
  } else {
    const count = Math.min(args.length, Math.max(...counts))
    if (!counts.includes(count) || count === 1 && !isObject(args[0])) {
      throw new TypeError(`Invalid ${name}(): ${args.length} arguments.`)
    }
  }
}

function isObject(value) {
  return value !== null && typeof value === 'object'
}

// A canvas's width or height, as WebIDL's `[EnforceRange] unsigned long
// long` takes it: a whole number from 0, from text too (`'0x96'`).
function getSize(value) {
  const size = Math.trunc(Number(value))
  if (!(size >= 0 && size <= Number.MAX_SAFE_INTEGER)) {
    throw new TypeError(`Invalid canvas size: '${value}'.`)
  }
  return size
}

// A side of an `ImageData`: a whole number of pixels, more than none.
function getSide(value) {
  const side = Math.trunc(Number(value))
  if (!(side > 0)) {
    throw new DOMException(
      `Invalid ImageData size: '${value}'.`,
      'IndexSizeError'
    )
  }
  return side
}
