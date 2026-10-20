import { checkArguments, setToStringTag } from './define.js'
import { EventTarget } from './events.js'
import { natives } from './natives.js'

// `navigator.clipboard` for scripts outside a browser: the app reads and
// writes the system clipboard's text for the script, as it answers its dialogs.
// Without the app (headless), it's denied with a `NotAllowedError`.

export class Clipboard extends EventTarget {
  async readText() {
    return ask({ type: 'clipboardRead' })?.text ?? ''
  }

  async writeText(text) {
    checkArguments(arguments, 1, 'writeText')
    ask({ type: 'clipboardWrite', text: String(text) })
  }
}

setToStringTag(Clipboard)

function ask(question) {
  try {
    return natives.ask(question)
  } catch (err) {
    throw new DOMException(
      `Unable to use the clipboard: ${err.message}`,
      'NotAllowedError'
    )
  }
}
