import {
  checkArguments,
  checkInternal,
  inject,
  internal,
  setToStringTag
} from './define.js'
import { natives } from './natives.js'

// `OffscreenCanvas` and its kin for scripts outside a browser, with pixels. A
// canvas is its own 2D context (`getContext('2d')` returns it), which draws as
// the browser's `CanvasRenderingContext2D` does. The runtime keeps the pixels,
// paths, gradients, patterns and drawing state in slots, and draws
// (`natives.canvas`); each object here keeps the number of its slot, and frees
// it when the script drops the object.

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

export class OffscreenCanvas {
  #surface

  // Transparent black, with a context's defaults.
  constructor(width, height) {
    this.#surface = canvas.create(width, height)
    registry?.register(this, ['surface', this.#surface])
  }

  static {
    getCanvasSurface = canvas => canvas.#surface
  }

  // The canvas is its own context, of any kind asked for:
  getContext() {
    return this
  }

  get canvas() {
    return this
  }

  get width() {
    return canvas.width.call(this.#surface)
  }

  // Cleared to the new size, its state reset, as in browsers:
  set width(width) {
    canvas.resize.call(this.#surface, width, undefined)
  }

  get height() {
    return canvas.height.call(this.#surface)
  }

  set height(height) {
    canvas.resize.call(this.#surface, undefined, height)
  }

  async convertToBlob() {
    const png = canvas.toPNG.call(this.#surface)
    return new globalThis.Blob([png], { type: 'image/png' })
  }

  // Transparent black, of a size or of another `ImageData`'s.
  createImageData(widthOrImage, height) {
    return widthOrImage instanceof ImageData
      ? new ImageData(widthOrImage.width, widthOrImage.height)
      : new ImageData(widthOrImage, height)
  }

  createLinearGradient(x0, y0, x1, y1) {
    return createGradient('linear', x0, y0, x1, y1)
  }

  createRadialGradient(x0, y0, r0, x1, y1, r1) {
    return createGradient('radial', x0, y0, r0, x1, y1, r1)
  }

  createConicGradient(angle, x, y) {
    return createGradient('conic', angle, x, y)
  }

  createPattern(image, repetition) {
    return new CanvasPattern(
      internal,
      canvas.createPattern(getSurface(image), repetition)
    )
  }

  drawImage(image, ...args) {
    canvas.methods.drawImage.call(this.#surface, getSurface(image), ...args)
  }

  // The path first, `null` for the current one:
  fill(...args) {
    return callWithPath(this.#surface, 'fill', args)
  }

  stroke(...args) {
    return callWithPath(this.#surface, 'stroke', args)
  }

  clip(...args) {
    return callWithPath(this.#surface, 'clip', args)
  }

  isPointInPath(...args) {
    return callWithPath(this.#surface, 'isPointInPath', args)
  }

  isPointInStroke(...args) {
    return callWithPath(this.#surface, 'isPointInStroke', args)
  }
}

// What the context does with numbers and text alone:
for (const name of [
  'getImageData',
  'putImageData',
  'measureText',
  'beginPath',
  'closePath',
  'moveTo',
  'lineTo',
  'bezierCurveTo',
  'quadraticCurveTo',
  'arc',
  'arcTo',
  'ellipse',
  'rect',
  'roundRect',
  'fillRect',
  'strokeRect',
  'clearRect',
  'fillText',
  'strokeText',
  'translate',
  'rotate',
  'scale',
  'transform',
  'setTransform',
  'resetTransform',
  'getTransform',
  'save',
  'restore',
  'reset',
  'setLineDash',
  'getLineDash'
]) {
  inject(OffscreenCanvas.prototype, {
    [name](...args) {
      return canvas.methods[name].call(getSurface(this), ...args)
    }
  })
}

// Its properties, with the slot of a gradient or a pattern given as a style:
for (const name of canvas.properties) {
  inject(
    OffscreenCanvas.prototype,
    {
      get [name]() {
        return canvas.get.call(getSurface(this), name)
      },

      set [name](value) {
        canvas.set.call(getSurface(this), name, value, getPaint(value))
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
  inject(Path2D.prototype, {
    [name](...args) {
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
// one, for the current path.
function callWithPath(surface, name, args) {
  const path = args[0] instanceof Path2D ? getPath(args.shift()) : null
  return canvas.methods[name].call(surface, path, ...args)
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
