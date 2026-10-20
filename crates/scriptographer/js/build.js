// Bundle the sources into the scripts the runtime loads: one for every
// platform, and one more for the web. `--check` only compares them with what's
// in dist/, for CI.
import { build } from 'esbuild'
import { readFile, writeFile } from 'node:fs/promises'

const bundles = {
  'dist/scriptographer.js': 'src/index.js',
  'dist/scriptographer-web.js': 'src/web/index.js'
}
const isCheck = process.argv.includes('--check')
const stale = []

for (const [file, entry] of Object.entries(bundles)) {
  const result = await build({
    entryPoints: [entry],
    bundle: true,
    format: 'iife',
    target: 'es2022',
    legalComments: 'inline',
    write: false
  })
  const text = result.outputFiles[0].text
  if (isCheck) {
    const current = await readFile(file, 'utf8').catch(() => null)
    if (current !== text) stale.push(file)
  } else {
    await writeFile(file, text)
  }
}

if (stale.length) {
  console.error(`Out of date, run 'pnpm run build': ${stale.join(', ')}`)
  process.exit(1)
}
