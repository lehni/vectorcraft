// Scriptographer's file dialogs, over the standard pickers: each gives a
// `LocalFile`, or `null` when the user cancels.
export function setupFileDialogs(Dialog) {
  Object.assign(Dialog, {
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
