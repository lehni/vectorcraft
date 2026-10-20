// What scripts see that's written in JavaScript, on every platform. The runtime
// runs it once its own classes are there.
import { setupFileDialogs } from './dialog.js'

setupFileDialogs(globalThis.Dialog)
