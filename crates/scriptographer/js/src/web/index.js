// What scripts see on the web that's written in JavaScript. The runtime passes
// what it needs as `__scriptographer`, which the bundle for every platform takes
// off the global object after this one.
import { LocalFile } from './local-file.js'
import { setupPickers } from './pickers.js'

const { ask } = globalThis.__scriptographer

setupPickers(ask)
globalThis.LocalFile = LocalFile

// Dialog's `alert()` and `confirm()` are the browser's, which shows no title of
// the script's: a title the script gave heads the message.
const getText = (...args) =>
  args.length > 1 ? `${args[0]}\n\n${args[1]}` : String(args[0] ?? '')

globalThis.Dialog = {
  alert(...args) {
    globalThis.alert(getText(...args))
  },

  confirm(...args) {
    return globalThis.confirm(getText(...args))
  }
}
