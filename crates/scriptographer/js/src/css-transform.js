// CSS's `<transform-list>`, as `new DOMMatrix(text)` reads it in a window: the
// transform functions, their lengths in pixels (no relative ones: there's no
// element to measure), angles in degrees, and `calc()` of them. What it can't
// read is a `SyntaxError`.

// The transform functions by their names, as CSS compares them (lower case),
// with the types they take: a type ending in `?` may be left out.
const functions = {
  matrix: Array(6).fill('number'),
  matrix3d: Array(16).fill('number'),
  translate: ['length', 'length?'],
  translatex: ['length'],
  translatey: ['length'],
  translatez: ['length'],
  translate3d: ['length', 'length', 'length'],
  scale: ['scale', 'scale?'],
  scalex: ['scale'],
  scaley: ['scale'],
  scalez: ['scale'],
  scale3d: ['scale', 'scale', 'scale'],
  rotate: ['angle'],
  rotatex: ['angle'],
  rotatey: ['angle'],
  rotatez: ['angle'],
  rotate3d: ['number', 'number', 'number', 'angle'],
  skew: ['angle', 'angle?'],
  skewx: ['angle'],
  skewy: ['angle'],
  perspective: ['length']
}

// The units CSS's values come in, as `[type, factor]` to pixels or degrees:
const units = {
  '': ['number', 1],
  '%': ['percentage', 1],
  'px': ['length', 1],
  'cm': ['length', 96 / 2.54],
  'mm': ['length', 96 / 25.4],
  'q': ['length', 96 / 101.6],
  'in': ['length', 96],
  'pc': ['length', 16],
  'pt': ['length', 4 / 3],
  'deg': ['angle', 1],
  'rad': ['angle', 180 / Math.PI],
  'grad': ['angle', 0.9],
  'turn': ['angle', 360]
}

// The functions of `text`, each `{ name, args }`, in order; none for `none`.
export function parseTransformList(text) {
  // Comments are as nothing, but leave nothing to read on their own:
  let rest = String(text)
    .replace(/\/\*[^]*?\*\//g, '')
    .trim()
  if (/^none$/i.test(rest)) return []
  const list = []
  do {
    const match = /^([a-z0-9]+)\(/i.exec(rest)
    const name = match?.[1].toLowerCase()
    const types = functions[name]
    if (!types) throw syntaxError(text)
    const end = getClosing(rest, match[0].length, text)
    const args = split(rest.slice(match[0].length, end)).map(arg =>
      parseValue(arg, text)
    )
    list.push({ name, args: getArguments(args, types, text) })
    rest = rest.slice(end + 1).trimStart()
  } while (rest)
  return list
}

// The numbers of `args` as `types` take them, or a `SyntaxError`.
function getArguments(args, types, text) {
  const needed = types.filter(type => !type.endsWith('?')).length
  if (args.length < needed || args.length > types.length) {
    throw syntaxError(text)
  }
  return args.map(({ type, value }, index) => {
    const wanted = types[index].replace('?', '')
    if (
      type === wanted ||
      // A length or an angle may be a plain 0:
      type === 'number' && value === 0 && wanted !== 'number'
    ) {
      return value
    } else if (
      wanted === 'scale' &&
      (type === 'number' || type === 'percentage')
    ) {
      return type === 'percentage' ? value / 100 : value
    } else {
      throw syntaxError(text)
    }
  })
}

// One value, `{ type, value }`: a number with its unit, or `calc()` of them.
function parseValue(source, text) {
  const tokens = tokenize(source.trim(), text)
  let at = 0
  const peek = () => tokens[at]
  const next = () => tokens[at++]

  // `a + b` and `a - b`, of values of the same type:
  const sum = () => {
    let left = product()
    while (peek() === '+' || peek() === '-') {
      const sign = next() === '+' ? 1 : -1
      const right = product()
      if (left.type !== right.type) throw syntaxError(text)
      left = { type: left.type, value: left.value + sign * right.value }
    }
    return left
  }

  // `a * b`, one of them a number, and `a / b`, `b` a number:
  const product = () => {
    let left = term()
    while (peek() === '*' || peek() === '/') {
      const operator = next()
      const right = term()
      if (operator === '*' && left.type === 'number') {
        left = { type: right.type, value: left.value * right.value }
      } else if (right.type === 'number') {
        left = {
          type: left.type,
          value:
            operator === '*'
              ? left.value * right.value
              : left.value / right.value
        }
      } else {
        throw syntaxError(text)
      }
    }
    return left
  }

  const term = () => {
    const token = next()
    if (token === '(' || token === 'calc(') {
      const value = sum()
      if (next() !== ')') throw syntaxError(text)
      return value
    } else if (token?.type) {
      return token
    } else {
      throw syntaxError(text)
    }
  }

  // Outside `calc()`, only a single value:
  const value = tokens[0] === 'calc(' ? term() : next()
  if (!value?.type || at !== tokens.length) throw syntaxError(text)
  return value
}

// The tokens of a value: numbers with their units as `{ type, value }`,
// `calc(`, parentheses, and operators, `+` and `-` between spaces.
function tokenize(source, text) {
  const tokens = []
  const pattern =
    /\s*(?:([+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?)([a-z%]*)|(calc\()|([()*/])|(\s[+-]\s))/iy
  while (pattern.lastIndex < source.length) {
    const match = pattern.exec(source)
    if (!match) throw syntaxError(text)
    const [, number, unit, calc, symbol, operator] = match
    if (number !== undefined) {
      const known = units[unit.toLowerCase()]
      if (!known) throw syntaxError(text)
      const [type, factor] = known
      tokens.push({ type, value: Number(number) * factor })
    } else {
      tokens.push(calc?.toLowerCase() ?? symbol ?? operator.trim())
    }
  }
  return tokens
}

// The index of the `)` that closes the `(` before `start`.
function getClosing(source, start, text) {
  let depth = 1
  for (let index = start; index < source.length; index++) {
    if (source[index] === '(') depth++
    else if (source[index] === ')' && --depth === 0) return index
  }
  throw syntaxError(text)
}

// The arguments of a function, split at its commas, not those inside `calc()`.
function split(source) {
  const args = []
  let depth = 0
  let start = 0
  for (let index = 0; index < source.length; index++) {
    const char = source[index]
    if (char === '(') depth++
    else if (char === ')') depth--
    else if (char === ',' && depth === 0) {
      args.push(source.slice(start, index))
      start = index + 1
    }
  }
  args.push(source.slice(start))
  return args
}

function syntaxError(text) {
  return new DOMException(
    `Invalid transform list: '${text}'.`,
    'SyntaxError'
  )
}
