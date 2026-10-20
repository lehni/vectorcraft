import { getComponents, getValue } from './dialog.js'
import { setToStringTag } from './define.js'

// Scriptographer's `Palette`: `new Palette(title, components, values)` is a
// panel that stays open while the script runs, with the same components as
// `Dialog.prompt()`. What the user changes goes into `values` and calls the
// component's `onChange(value)`, then the palette's `onChange(component)`; a
// button calls its `onClick()`, and a menu entry, in the palette's menu, its
// `onSelect()`. What the script changes in `values` or in the components shows
// in the panel.

// The open palettes by their numbers, with the components as defined, for
// `reset()`, and what was last reported of them:
const palettes = new Map()
let lastId = 0

export class Palette {
  constructor(title = '', components, values) {
    if (!components || typeof components !== 'object') {
      throw new TypeError('Invalid components: they must be an object.')
    }
    this.title = String(title)
    this.components = components
    this.values = values && typeof values === 'object' ? values : {}
    // `values` holds every component's value from the start:
    for (const component of getComponents(components, this.values)) {
      if (this.values[component.name] === undefined) {
        this.values[component.name] = getValue(component, component.value)
      }
    }
    palettes.set(++lastId, {
      palette: this,
      defaults: getComponents(components),
      reported: ''
    })
  }

  // Every component's value back to the one it was defined with.
  reset() {
    const { defaults } = [...palettes.values()].find(
      entry => entry.palette === this
    )
    for (const component of defaults) {
      const value = getValue(component, component.value)
      this.values[component.name] = value
      // Handlers read a component's value from its definition:
      this.components[component.name].value = value
    }
  }
}

setToStringTag(Palette)

// The palettes that are new or changed since the last call, as the app shows
// them: `{ id, title, components }`.
export function takePalettes() {
  const changed = []
  for (const [id, entry] of palettes) {
    const state = getState(id, entry.palette)
    const text = JSON.stringify(state)
    if (entry.reported !== text) {
      entry.reported = text
      changed.push(state)
    }
  }
  return changed
}

export function hasPalettes() {
  return palettes.size > 0
}

// What the user did in palette `id`: `change` a component's value, `click` a
// button or menu entry, or `close` it.
export function handlePaletteEvent(id, type, name, value) {
  const entry = palettes.get(id)
  if (entry) {
    const { palette } = entry
    const { components, values } = palette
    const definition = components[name]
    if (type === 'change') {
      const component = getComponents(components, values).find(
        component => component.name === name
      )
      if (component) {
        value = getValue(component, value)
        values[name] = value
        // The panel shows this value already: only what the handlers change
        // goes back.
        entry.reported = JSON.stringify(getState(id, palette))
        // Handlers get the component with its name and value, as
        // Scriptographer's had them:
        definition.name = name
        definition.value = value
        call(definition, 'onChange', value)
        call(palette, 'onChange', definition)
      }
    } else if (type === 'click') {
      call(definition, 'onClick')
      call(definition, 'onSelect')
    } else if (type === 'close') {
      palettes.delete(id)
      call(palette, 'onClose')
    }
  }
}

function getState(id, palette) {
  const { title, components, values } = palette
  return { id, title, components: getComponents(components, values) }
}

function call(owner, name, ...args) {
  if (typeof owner?.[name] === 'function') owner[name](...args)
}
