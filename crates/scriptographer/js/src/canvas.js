import {
  checkArguments,
  checkInternal,
  inject,
  setToStringTag
} from './define.js'
import { natives } from './natives.js'

// `OffscreenCanvas` and its kin for scripts outside a browser, with pixels. A
// canvas is its own 2D context (`getContext('2d')` returns it), which draws as
// the browser's `CanvasRenderingContext2D` does. The runtime keeps the pixels,
// the paths and the drawing state, and draws (`natives.canvas`); the classes
// here are what scripts see of them.

const { canvas } = natives

export class OffscreenCanvas {
  // Transparent black, with a context's defaults.
  constructor(width, height) {
    canvas.create.call(this, width, height)
  }

  // The canvas is its own context, of any kind asked for:
  getContext() {
    return this
  }

  get canvas() {
    return this
  }

  get width() {
    return canvas.width.call(this)
  }

  // Cleared to the new size, its state reset, as in browsers:
  set width(width) {
    canvas.resize.call(this, width, undefined)
  }

  get height() {
    return canvas.height.call(this)
  }

  set height(height) {
    canvas.resize.call(this, undefined, height)
  }

  // Transparent black, of a size or of another `ImageData`'s.
  createImageData(widthOrImage, height) {
    return widthOrImage instanceof ImageData
      ? new ImageData(widthOrImage.width, widthOrImage.height)
      : new ImageData(widthOrImage, height)
  }
}

inject(OffscreenCanvas.prototype, canvas.methods)
for (const name of canvas.properties) {
  inject(
    OffscreenCanvas.prototype,
    {
      get [name]() {
        return canvas.get.call(this, name)
      },

      set [name](value) {
        canvas.set.call(this, name, value)
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
  constructor(path) {
    canvas.createPath.call(this, path)
  }
}

inject(Path2D.prototype, canvas.pathMethods)

// Decoded images, or copies of an `ImageData`, a canvas or another bitmap, as
// `createImageBitmap()` makes them, to draw with `drawImage()` and
// `createPattern()`; `close()` frees its pixels.
export class ImageBitmap {
  constructor(key) {
    checkInternal(key, 'ImageBitmap', 'createImageBitmap() makes them')
  }

  get width() {
    return canvas.bitmapWidth.call(this)
  }

  get height() {
    return canvas.bitmapHeight.call(this)
  }

  close() {
    canvas.closeBitmap.call(this)
  }
}

export class CanvasGradient {
  constructor(key) {
    checkInternal(key, 'CanvasGradient', 'the context makes them')
  }

  addColorStop(offset, color) {
    canvas.addColorStop.call(this, offset, color)
  }
}

export class CanvasPattern {
  constructor(key) {
    checkInternal(key, 'CanvasPattern', 'the context makes them')
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

export const { createImageBitmap } = canvas

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
