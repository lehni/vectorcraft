import {
  checkArguments,
  checkInternal,
  inject,
  internal,
  setToStringTag
} from './define.js'
import { DOMRect, DOMRectReadOnly } from './geometry.js'

// What `measureText()` gives: the numbers the runtime measured, from the point
// `textAlign` and `textBaseline` put at the text's position, and the text's
// grapheme clusters, by which `getTextClusters()`, `getActualBoundingBox()`,
// `getSelectionRects()` and `getIndexFromOffset()` answer.

// The numbers the runtime measured, by name:
const numbers = [
  'width',
  'actualBoundingBoxLeft',
  'actualBoundingBoxRight',
  'actualBoundingBoxAscent',
  'actualBoundingBoxDescent',
  'fontBoundingBoxAscent',
  'fontBoundingBoxDescent',
  'emHeightAscent',
  'emHeightDescent',
  'hangingBaseline',
  'alphabeticBaseline',
  'ideographicBaseline'
]

// The context's settings a cluster is drawn with, as they were measured:
export const textSettings = [
  'font',
  'letterSpacing',
  'wordSpacing',
  'fontKerning',
  'fontVariantCaps',
  'direction',
  'lang'
]

let getValues, getClusterSettings

export class TextMetrics {
  #values
  #text
  #settings
  // Each cluster's start and end, left and advance, and its ink's left, right,
  // top and bottom (heights):
  #clusters = []

  constructor(key, values, text, settings) {
    checkInternal(key, 'TextMetrics', 'measureText() makes them')
    this.#values = values
    this.#text = text
    this.#settings = settings
    const { clusters } = values
    for (let at = 0; at < clusters.length; at += 8) {
      const [start, end, left, advance, inkLeft, inkRight, top, bottom] =
        clusters.subarray(at, at + 8)
      this.#clusters.push({
        start,
        end,
        left,
        advance,
        inkLeft,
        inkRight,
        top,
        bottom
      })
    }
  }

  static {
    getValues = metrics => metrics.#values
  }

  // The clusters of the text from `start` to `end`, positioned by `align` and
  // `baseline`, by default the context's when it was measured.
  getTextClusters(...args) {
    const [start, end, options = {}] =
      args.length > 1 ? args : [0, this.#text.length, args[0]]
    const {
      align = this.#settings.textAlign,
      baseline = this.#settings.textBaseline
    } = options
    const y = -this.#getBaselineHeight(baseline)
    return this.#getClusters(start, end, 'getTextClusters').map(
      cluster =>
        new TextCluster(
          internal,
          {
            text: this.#text.slice(cluster.start, cluster.end),
            x: (
              cluster.left +
              getAlignOffset(align, cluster.advance, this.#settings)
            ),
            y,
            start: cluster.start,
            end: cluster.end,
            align,
            baseline
          },
          this.#settings
        )
    )
  }

  // The bounds of the ink of the text from `start` to `end`:
  getActualBoundingBox(start, end) {
    checkArguments(arguments, 2, 'getActualBoundingBox')
    const inked = this.#getClusters(start, end, 'getActualBoundingBox').filter(
      cluster => cluster.inkRight > cluster.inkLeft
    )
    if (!inked.length) {
      return new DOMRectReadOnly()
    }
    const left = Math.min(...inked.map(cluster => cluster.inkLeft))
    const right = Math.max(...inked.map(cluster => cluster.inkRight))
    const top = Math.max(...inked.map(cluster => cluster.top))
    const bottom = Math.min(...inked.map(cluster => cluster.bottom))
    return new DOMRectReadOnly(left, -top, right - left, top - bottom)
  }

  // The rectangles that select the text from `start` to `end`, as tall as the
  // font's bounding box:
  getSelectionRects(start, end) {
    checkArguments(arguments, 2, 'getSelectionRects')
    const { fontBoundingBoxAscent: ascent, fontBoundingBoxDescent: descent } =
      this.#values
    const rects = []
    for (const { left, advance } of this.#getClusters(
      start,
      end,
      'getSelectionRects'
    )) {
      const last = rects.at(-1)
      if (last && Math.abs(last.right - left) < 1e-6) {
        last.width += advance
      } else {
        rects.push(new DOMRect(left, -ascent, advance, ascent + descent))
      }
    }
    return rects
  }

  // The index of the character position nearest to `offset`, from the text's
  // position: before a cluster up to its middle, after it beyond, in the
  // cluster's own direction, which the next character shows.
  getIndexFromOffset(offset) {
    checkArguments(arguments, 1, 'getIndexFromOffset')
    offset = Number(offset)
    const clusters = this.#clusters
    const isRightToLeft = index => {
      const [cluster, next, previous] = [index, index + 1, index - 1].map(
        i => clusters[i]
      )
      return next
        ? next.left < cluster.left
        : previous
          ? previous.left > cluster.left
          : this.#settings.direction === 'rtl'
    }
    const visual = clusters
      .map((cluster, index) => ({ ...cluster, rtl: isRightToLeft(index) }))
      .sort((a, b) => a.left - b.left)
    for (const { start, end, left, advance, rtl } of visual) {
      if (offset <= left + advance / 2) {
        return rtl ? end : start
      }
    }
    return this.#settings.direction === 'rtl' ? 0 : this.#text.length
  }

  // A start after the end selects nothing for `getSelectionRects()`.
  #getClusters(start, end, name) {
    start = getIndex(start, name)
    end = getIndex(end, name)
    const { length } = this.#text
    if (
      start > length ||
      end > length ||
      (start > end && name !== 'getSelectionRects')
    ) {
      throw new DOMException(
        `Invalid ${name}(): ${start} to ${end} isn't in the text.`,
        'IndexSizeError'
      )
    }
    return this.#clusters.filter(
      cluster => cluster.start < end && cluster.end > start
    )
  }

  // The height of the baseline above the text's position:
  #getBaselineHeight(baseline) {
    const values = this.#values
    return (
      {
        top: values.emHeightAscent,
        hanging: values.hangingBaseline,
        middle: (values.emHeightAscent - values.emHeightDescent) / 2,
        alphabetic: values.alphabeticBaseline,
        ideographic: values.ideographicBaseline,
        bottom: -values.emHeightDescent
      }[baseline] ??
      values.alphabeticBaseline
    )
  }
}

for (const name of numbers) {
  inject(
    TextMetrics.prototype,
    {
      get [name]() {
        return getValues(this)[name]
      }
    },
    { enumerable: true }
  )
}

// A grapheme cluster of a `TextMetrics`, which `fillTextCluster()` and
// `strokeTextCluster()` draw with the context's settings it was measured with.
export class TextCluster {
  #values
  #settings

  constructor(key, values, settings) {
    checkInternal(key, 'TextCluster', 'getTextClusters() makes them')
    this.#values = values
    this.#settings = settings
  }

  static {
    getClusterSettings = cluster => cluster.#settings
  }

  get text() {
    return this.#values.text
  }

  get x() {
    return this.#values.x
  }

  get y() {
    return this.#values.y
  }

  get start() {
    return this.#values.start
  }

  get end() {
    return this.#values.end
  }

  get align() {
    return this.#values.align
  }

  get baseline() {
    return this.#values.baseline
  }
}

setToStringTag(TextMetrics, TextCluster)

// Draws `cluster` with the context's `draw` (`fillText` or `strokeText`) at
// `(x, y)`, moved by its `x` and `y` and placed by its `align` and `baseline`,
// unless `options` say otherwise.
export function drawCluster(context, draw, cluster, x, y, options = {}) {
  if (!(cluster instanceof TextCluster)) {
    throw new TypeError(`Invalid text cluster: '${cluster}'.`)
  }
  const {
    align = cluster.align,
    baseline = cluster.baseline,
    x: dx = cluster.x,
    y: dy = cluster.y
  } = options
  context.save()
  Object.assign(context, getClusterSettings(cluster), {
    textAlign: align,
    textBaseline: baseline
  })
  context[draw](cluster.text, Number(x) + dx, Number(y) + dy)
  context.restore()
}

// Where `align` puts a cluster's position, from its left:
function getAlignOffset(align, advance, { direction }) {
  const rtl = direction === 'rtl'
  return align === 'center'
    ? advance / 2
    : (
          align === 'right' ||
          (align === 'end' && !rtl) ||
          (align === 'start' && rtl)
        )
      ? advance
      : 0
}

// An index as WebIDL's `[EnforceRange] unsigned long` takes it.
function getIndex(value, name) {
  const index = Math.trunc(Number(value))
  if (!(index >= 0 && index <= 2 ** 32 - 1)) {
    throw new TypeError(`Invalid ${name}(): '${value}' isn't an index.`)
  }
  return index
}
