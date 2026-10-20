import { formatError } from './format-error.js'

// `console` as browsers have it, printing to the script's output, and
// Scriptographer's `print()`, which logs. Values read as a browser's console
// shows them: strings at the top as they are, arrays as `[1, 2]`, plain objects
// as `{a: 1}`, functions as `ƒ name()`, errors with their stack, and anything
// with a `toString()` of its own (Paper.js's `Point`) as it writes itself.

// How deep arrays and objects are written, and how many of their items:
const maxDepth = 2
const maxItems = 100

export function createConsole(print) {
  let indent = 0
  const counts = new Map()
  const timers = new Map()

  // Each line indented as deep as the groups go:
  const write = text =>
    print(
      text
        .split('\n')
        .map(line => '  '.repeat(indent) + line)
        .join('\n')
    )

  const log = (...args) => write(format(args))
  const now = () => globalThis.performance?.now() ?? Date.now()

  // `timeLog()`, or `timeEnd()` (`end`), which stops the timer.
  const timeLog = (end, label = 'default', ...data) => {
    label = String(label)
    if (timers.has(label)) {
      let line = `${label}: ${(now() - timers.get(label)).toFixed(3)} ms`
      if (end) timers.delete(label)
      else if (data.length) line += ` ${format(data)}`
      write(line)
    } else {
      write(`Timer '${label}' does not exist`)
    }
  }

  const group = (...args) => {
    if (args.length) log(...args)
    indent++
  }

  return {
    log,
    info: log,
    warn: log,
    error: log,
    debug: log,
    dirxml: log,

    // The value as an object, strings quoted.
    dir(value) {
      write(inspect(value, 1))
    },

    // The line, then where it was called from.
    trace(...args) {
      const line = format(args)
      // The stack without the error's own line:
      const stack = (new Error().stack ?? '')
        .split('\n')
        .slice(1)
        .map(line => line.trim())
        .join('\n')
      write((line ? `Trace: ${line}` : 'Trace') + (stack ? `\n${stack}` : ''))
    },

    assert(condition, ...data) {
      if (!condition) {
        write(
          data.length ? `Assertion failed: ${format(data)}` : 'Assertion failed'
        )
      }
    },

    count(label = 'default') {
      label = String(label)
      const count = (counts.get(label) ?? 0) + 1
      counts.set(label, count)
      write(`${label}: ${count}`)
    },

    countReset(label = 'default') {
      label = String(label)
      if (counts.has(label)) {
        counts.set(label, 0)
      } else {
        write(`Count for '${label}' does not exist`)
      }
    },

    time(label = 'default') {
      label = String(label)
      if (timers.has(label)) {
        write(`Timer '${label}' already exists`)
      } else {
        timers.set(label, now())
      }
    },

    timeLog(label, ...data) {
      timeLog(false, label, ...data)
    },

    timeEnd(label) {
      timeLog(true, label)
    },

    group,
    groupCollapsed: group,

    groupEnd() {
      indent = Math.max(0, indent - 1)
    },

    // An array's or object's items as rows, their properties as columns.
    table(data, columns) {
      if (data && typeof data === 'object') {
        write(formatTable(data, columns))
      } else {
        log(data)
      }
    },

    clear() {}
  }
}

// The arguments as one line: the first one's `%` directives filled in with
// those after it, the rest after it, as consoles write them.
function format(args) {
  const parts = []
  let rest = args
  const [first, ...after] = args
  if (typeof first === 'string' && first.includes('%')) {
    rest = after
    parts.push(
      first.replace(/%([sdifoOc%])/g, (match, directive) => {
        if (directive === '%') return '%'
        if (!rest.length) return match
        const value = rest.shift()
        switch (directive) {
          case 's':
            return inspect(value, 0)
          case 'd':
          case 'i': {
            const number = Number(value)
            return String(Number.isFinite(number) ? Math.trunc(number) : number)
          }
          case 'f':
            return String(Number(value))
          case 'o':
          case 'O':
            return inspect(value, 1)
          default:
            // CSS for the line, which the output can't show.
            return ''
        }
      })
    )
  }
  for (const value of rest) parts.push(inspect(value, 0))
  return parts.join(' ')
}

// `value` as a console writes it, `depth` deep in arrays and objects, where
// strings are quoted.
function inspect(value, depth) {
  if (typeof value === 'string') {
    return depth === 0 ? value : `'${value}'`
  }
  if (
    value === null ||
    typeof value !== 'object' && typeof value !== 'function'
  ) {
    return String(value)
  }
  const tag = Object.prototype.toString.call(value)
  if (tag === '[object Array]') {
    return depth > maxDepth
      ? '[Array]'
      : `[${
          getItems(
            Array.from({ length: Math.min(value.length, maxItems) }, (_, i) => [
              i,
              value[i]
            ]),
            value.length,
            (_, item) => inspect(item, depth + 1)
          )
        }]`
  } else if (/^\[object (Async|Generator)?Function\]$/.test(tag)) {
    return `ƒ ${value.name}()`
  } else if (tag === '[object Error]') {
    return formatError(value)
  } else {
    // An object that writes itself (`toString()` of its own) is written so.
    const written = String(value)
    if (!written.startsWith('[object ')) {
      return written
    } else if (depth > maxDepth) {
      return '{…}'
    } else {
      // Named for its class, unless it's a plain object.
      const name = value.constructor?.name
      const prefix = name && name !== 'Object' ? `${name} ` : ''
      const keys = Object.keys(value)
      const members = getItems(
        keys.slice(0, maxItems).map(key => [key, value[key]]),
        keys.length,
        (key, member) => `${key}: ${inspect(member, depth + 1)}`
      )
      return `${prefix}{${members}}`
    }
  }
}

// The first entries of `total`, written by `write(key, value)`, and how many
// more there are.
function getItems(entries, total, write) {
  const written = entries.map(([key, value]) => write(key, value))
  if (total > entries.length) {
    written.push(`… ${total - entries.length} more`)
  }
  return written.join(', ')
}

function formatTable(data, chosen) {
  let columns = []
  const rows = Object.keys(data)
    .slice(0, maxItems)
    .map(key => {
      const row = data[key]
      if (row !== null && typeof row === 'object') {
        const cells = {}
        for (const column of Object.keys(row).slice(0, maxItems)) {
          cells[column] = inspect(row[column], 1)
          if (!columns.includes(column)) columns.push(column)
        }
        return { key, cells }
      } else {
        return { key, cells: {}, value: inspect(row, 1) }
      }
    })
  // Only the columns asked for, when they are:
  if (chosen && typeof chosen === 'object') {
    columns = Array.from(chosen, String)
  }
  const hasValues = rows.some(row => 'value' in row)
  const grid = [
    ['(index)', ...columns, ...(hasValues ? ['Value'] : [])],
    ...rows.map(({ key, cells, value }) => [
      key,
      ...columns.map(column => cells[column] ?? ''),
      ...(hasValues ? [value ?? ''] : [])
    ])
  ]
  const widths = grid[0].map((_, i) =>
    Math.max(...grid.map(row => [...row[i]].length))
  )
  return grid
    .map(
      row =>
        `│ ${
          row
            .map((cell, i) => cell + ' '.repeat(widths[i] - [...cell].length))
            .join(' │ ')
        } │`
    )
    .join('\n')
}
