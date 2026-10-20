import { natives } from './natives.js'

// Scriptographer's `Dialog`. `alert()`, `confirm()`, `prompt()` and
// `chooseColor()` ask the app, which shows the dialog: on desktop, scripts wait
// for the answer, as they did in Scriptographer; on the web, where they can't,
// `prompt()` and `chooseColor()` give Promises, and `alert()` and `confirm()`
// are the browser's own (`web/index.js`). The file dialogs are over the
// standard pickers.

// The title of a dialog the script doesn't name:
const defaultTitle = 'Scriptographer'

export function setupDialog(Dialog) {
  // `([title,] message)`:
  Dialog.alert ??= function (...args) {
    natives.ask({ type: 'alert', ...getTexts(args) })
  }

  Dialog.confirm ??= function (...args) {
    return natives.ask({ type: 'confirm', ...getTexts(args) }) === true
  }

  Object.assign(Dialog, {
    // The values by name, put into `values` when given, or `null` when the
    // user cancels.
    prompt(title = defaultTitle, components, values) {
      if (!components || typeof components !== 'object') {
        throw new TypeError('Invalid components: they must be an object.')
      }
      const list = getComponents(components, values)
      return then(
        natives.ask({ type: 'prompt', title: String(title), components: list }),
        answer => getValues(list, answer, values)
      )
    },

    // A prompt with one colour: the colour picked, or `null`.
    chooseColor(color) {
      const component = { type: 'color' }
      if (color != null) component.value = color
      return then(
        Dialog.prompt(defaultTitle, { color: component }),
        values => values && values.color
      )
    },

    // Each gives a `LocalFile`, or `null` when the user cancels.
    async openFile(options) {
      const handles = await pick(
        showOpenFilePicker,
        getPickerOptions('openFile', options)
      )
      return handles && LocalFile.from(handles[0])
    },

    async openFiles(options) {
      const handles = await pick(showOpenFilePicker, {
        ...getPickerOptions('openFile', options),
        multiple: true
      })
      return (
        handles &&
        Promise.all(handles.map(handle => LocalFile.from(handle)))
      )
    },

    async saveFile(options) {
      const handle = await pick(
        showSaveFilePicker,
        getPickerOptions('saveFile', options)
      )
      return handle && LocalFile.from(handle)
    },

    async chooseDirectory(options) {
      const handle = await pick(
        showDirectoryPicker,
        getPickerOptions('chooseDirectory', options)
      )
      return handle && LocalFile.from(handle)
    }
  })
}

// The components as the app gets them: each definition as JSON with its
// `name`, its `type` filled in where the script left it out, and its `value`
// from `values` or the definition, a colour as CSS.
export function getComponents(components, values) {
  return Object.keys(components).map(name => {
    const definition = components[name]
    // In the same order however the handlers changed the definition:
    const component = { name, ...toJSON(definition) }
    delete component.value
    let value = values?.[name] !== undefined ? values[name] : definition.value
    if (typeof value?.toCSS === 'function') {
      value = value.toCSS(true)
    }
    component.value = toJSON(value)
    component.type ??=
      // As Scriptographer read it from the definition:
      'options' in component
        ? 'list'
        : typeof definition.onClick === 'function'
          ? 'button'
          : typeof definition.onSelect === 'function'
            ? 'menu-entry'
            : typeof component.value === 'number'
              ? 'number'
              : typeof component.value === 'boolean'
                ? 'boolean'
                : 'string'
    // A number with units has steppers unless it says otherwise:
    if (
      component.type === 'number' &&
      typeof component.units === 'string' &&
      component.units !== 'none'
    ) {
      component.steppers ??= true
    }
    return component
  })
}

// The script's value of `component`'s value from the app: a colour from CSS.
export function getValue(component, value) {
  return component.type === 'color' && typeof value === 'string'
    ? new paper.Color(value)
    : value
}

// The values of the user's `answer` to a prompt of the components `list`, put
// into `values` when given, or `null` when they cancelled.
function getValues(list, answer, values) {
  if (answer && typeof answer === 'object') {
    const result = values ?? {}
    for (const component of list) {
      if (component.name in answer) {
        result[component.name] = getValue(component, answer[component.name])
      }
    }
    return result
  } else {
    return null
  }
}

function getTexts(args) {
  const [title, message] =
    args.length > 1 ? args : [defaultTitle, args[0] ?? '']
  return { title: String(title), message: String(message) }
}

// `value` as JSON keeps it: functions and what JSON can't hold drop out.
function toJSON(value) {
  return JSON.parse(JSON.stringify(value ?? null) ?? 'null')
}

// `f(value)`, once `value` is there: on the web, the app answers later.
function then(value, f) {
  return value instanceof Promise ? value.then(f) : f(value)
}

async function pick(picker, options) {
  try {
    return await picker(options)
  } catch (err) {
    // Cancelling a picker rejects with an `AbortError`, a dialog gives `null`:
    if (err.name === 'AbortError') return null
    throw err
  }
}

// The pickers' options for Dialog's `{ types, path }`: its file types, and
// where to start, or for saving, which name to suggest.
function getPickerOptions(kind, { types, path } = {}) {
  const options = {}
  if (types && kind !== 'chooseDirectory') {
    const accepted = []
    let acceptsAll = false
    for (const [description, extensions] of getTypes(types)) {
      if (extensions.length) {
        accepted.push({
          description,
          accept: {
            'application/octet-stream': extensions.map(ext => `.${ext}`)
          }
        })
      } else {
        acceptsAll = true
      }
    }
    options.types = accepted
    options.excludeAcceptAllOption = !acceptsAll
  }
  if (path != null) {
    const file = path instanceof LocalFile ? path : getLocalFile(path)
    if (file?.path) {
      // Start in the directory, or in the file's, suggesting its name to save:
      options.startIn = file.isDirectory ? file : file.parent
      if (kind === 'saveFile' && !file.isDirectory) {
        options.suggestedName = file.name
      }
    } else if (kind === 'saveFile') {
      // Without paths (the web), only the name to suggest:
      options.suggestedName = String(path).split(/[\\/]/).pop()
    }
  }
  return options
}

// The file types as `[label, extensions]`, from `{ Label: 'ext' | [...] }` or
// Scriptographer's pairs of a label and its patterns, `['Text (*.txt)',
// '*.txt;*.text', ...]`, where a lone pattern labels itself. A type without
// extensions (`'*'`) shows every file.
function getTypes(types) {
  const entries = Array.isArray(types)
    ? Array.from({ length: Math.ceil(types.length / 2) }, (_, i) => [
        types[i * 2],
        types[i * 2 + 1] ?? types[i * 2]
      ])
    : Object.entries(types)
  return entries.map(([label, patterns]) => [
    String(label),
    (Array.isArray(patterns) ? patterns : String(patterns).split(';'))
      .map(pattern =>
        String(pattern)
          .trim()
          .replace(/^\*?\.?/, '')
      )
      .filter(ext => ext && ext !== '*')
  ])
}

function getLocalFile(path) {
  try {
    return new LocalFile(String(path))
  } catch {
    // The web has no paths.
    return null
  }
}
