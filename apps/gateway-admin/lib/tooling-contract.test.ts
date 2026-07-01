import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import assert from 'node:assert/strict'

const packageJson = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'))
const tsconfig = JSON.parse(readFileSync(new URL('../tsconfig.json', import.meta.url), 'utf8'))

test('gateway-admin tooling uses the documented bundler module contract', () => {
  assert.equal(packageJson.type, 'module')
  assert.equal(tsconfig.compilerOptions.module, 'esnext')
  assert.equal(tsconfig.compilerOptions.moduleResolution, 'bundler')
  assert.deepEqual(tsconfig.compilerOptions.paths['@/*'], ['./*'])
})

test('gateway-admin verification scripts exercise unit and browser test contracts', () => {
  assert.equal(packageJson.scripts.test, 'pnpm run test:unit && pnpm run test:install-script')
  assert.match(packageJson.scripts['test:unit'], /tsx --test/)
  assert.equal(packageJson.scripts['test:install-script'], 'node --test scripts/*.test.mjs')
  assert.equal(packageJson.scripts['test:browser'], 'node --test --experimental-strip-types lib/browser/**/*.test.ts')
})
