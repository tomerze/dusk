import { mkdir, readFile } from 'node:fs/promises'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { chromium } from 'playwright'
import { preview } from 'vite'

const axeSource = await readFile(
  createRequire(import.meta.url).resolve('axe-core/axe.min.js'),
  'utf8',
)
const output = resolve(process.argv[2] ?? join(tmpdir(), 'twilight-screens'))
const only = process.argv[3] === undefined ? null : new RegExp(process.argv[3])

const campaigns = {
  running: '01929b3e-7c4a-7d1e-9f3a-5b8c2d4e6f01',
  gatePaused: '01929b41-02aa-7b6e-8d11-0c7f3e9a2b02',
  converging: '01929a77-5d10-7e44-a2c9-1f6b0e8d3c03',
  sampling: '01929c02-9e3b-7a55-b6d0-4c2e8f1a7d04',
  draft: '01929d00-6f7a-7b8c-9d0e-1f2a3b4c5d07',
  archived: '019298a0-cfe0-7b3c-9d4e-6f7a8b9c0d13',
}

const pages = [
  { name: 'overview', path: '/' },
  { name: 'overview-degraded', path: '/?scenario=degraded' },
  { name: 'overview-empty', path: '/?scenario=empty' },
  {
    name: 'overview-error',
    path: '/?scenario=errors',
    prepare: async (page) => {
      await page.getByText('Could not load the overview').waitFor({ timeout: 30000 })
    },
  },
  { name: 'campaigns', path: '/campaigns' },
  { name: 'campaign-running', path: `/campaigns/${campaigns.running}` },
  { name: 'campaign-gate-paused', path: `/campaigns/${campaigns.gatePaused}` },
  { name: 'campaign-converging', path: `/campaigns/${campaigns.converging}` },
  { name: 'campaign-sampling', path: `/campaigns/${campaigns.sampling}` },
  { name: 'campaign-degraded', path: `/campaigns/${campaigns.running}?scenario=degraded` },
  { name: 'campaign-draft', path: `/campaigns/${campaigns.draft}` },
  { name: 'campaign-nodes', path: `/campaigns/${campaigns.gatePaused}?tab=nodes` },
  { name: 'campaign-history', path: `/campaigns/${campaigns.gatePaused}?tab=history` },
  { name: 'campaign-definition', path: `/campaigns/${campaigns.running}?tab=definition` },
  { name: 'campaign-archived', path: `/campaigns/${campaigns.archived}?tab=nodes` },
  { name: 'campaign-new', path: '/campaigns/new' },
  {
    name: 'campaign-new-selector-error',
    path: '/campaigns/new',
    prepare: async (page) => {
      const editor = page.getByRole('textbox', { name: 'Selector' })
      await editor.click()
      await page.keyboard.type('country == "US" and os_nme == "debian"')
      await page.getByText('unknown field').first().waitFor()
    },
  },
  {
    name: 'campaign-new-review',
    path: '/campaigns/new',
    prepare: async (page) => {
      await page
        .getByRole('textbox', { name: 'Name', exact: true })
        .fill('Dusk 0.2.2 to Northwind depots')
      const editor = page.getByRole('textbox', { name: 'Selector' })
      await editor.click()
      await page.keyboard.type('tenant == "northwind-logistics" and dusk_version < "0.2.2"')
      await page
        .getByText(/nodes match/)
        .first()
        .waitFor()
      await page.getByRole('radio', { name: /Ensure version/ }).check({ force: true })
      await page.getByRole('textbox', { name: 'Version', exact: true }).fill('0.2.2')
      const script = page.getByRole('textbox', { name: 'Script', exact: true })
      await script.click()
      await page.keyboard.type(
        'cp :/mnt/updates/dusk-node-0.2.2 :/var/lib/dusk/dusk-node.next\nkvs set dusk.update.pending 0.2.2',
      )
      await page.getByRole('button', { name: 'Review' }).click()
      await page.getByRole('heading', { name: 'Review' }).waitFor()
    },
  },
  { name: 'nodes', path: '/nodes' },
  {
    name: 'nodes-filtered',
    path: `/nodes?selector=${encodeURIComponent('os_build == "22631.4317"')}`,
  },
  { name: 'node', path: null },
  { name: 'alerts', path: '/alerts' },
  { name: 'alerts-all', path: '/alerts?state=all' },
  { name: 'login', path: '/?scenario=signed-out' },
  { name: 'not-found', path: '/no/such/page' },
  { name: 'viewer', path: `/campaigns/${campaigns.gatePaused}?scenario=viewer` },
]

const viewports = [
  { name: '1440', width: 1440, height: 900 },
  { name: '390', width: 390, height: 844 },
]

const schemes = ['dark', 'light']

async function settle(page) {
  await page.waitForLoadState('load')
  await page.waitForFunction(() => document.querySelector('#root')?.children.length)
  await page
    .waitForFunction(() => document.querySelectorAll('[aria-busy="true"]').length === 0, null, {
      timeout: 15000,
    })
    .catch(() => undefined)
  await page.waitForTimeout(900)
}

const server = await preview({
  mode: 'mock',
  build: { outDir: 'dist-mock' },
  preview: { port: 0, host: '127.0.0.1', strictPort: false },
  logLevel: 'warn',
})
const address = server.resolvedUrls?.local[0]
if (address === undefined) {
  throw new Error('vite preview did not report a local address')
}
await mkdir(output, { recursive: true })
const browser = await chromium.launch()
const written = []
const problems = []
try {
  let nodePath = null
  for (const scheme of schemes) {
    for (const viewport of viewports) {
      const context = await browser.newContext({
        viewport: { width: viewport.width, height: viewport.height },
        deviceScaleFactor: 1,
        colorScheme: scheme,
        reducedMotion: 'reduce',
      })
      await context.addInitScript((value) => {
        window.localStorage.setItem('twilight.color-scheme', value)
      }, scheme)
      for (const entry of pages) {
        if (only !== null && !only.test(entry.name)) {
          continue
        }
        const page = await context.newPage()
        const failures = []
        page.on('pageerror', (error) => failures.push(error.message))
        page.on('console', (message) => {
          if (message.type() === 'error') {
            failures.push(message.text())
          }
        })
        let path = entry.path
        if (path === null) {
          if (nodePath === null) {
            await page.goto(new URL('/nodes', address).href)
            await settle(page)
            const link = page.locator('a[href^="/nodes/"]').first()
            nodePath = await link.getAttribute('href')
          }
          path = nodePath
        }
        await page.goto(new URL(path, address).href)
        await settle(page)
        if (entry.prepare !== undefined) {
          await entry.prepare(page)
          await page.waitForTimeout(700)
        }
        const file = join(output, `${entry.name}-${scheme}-${viewport.name}.png`)
        await page.screenshot({ path: file, fullPage: true })
        written.push(file)
        await page.addScriptTag({ content: axeSource })
        const violations = await page.evaluate(async () => {
          const result = await window.axe.run(document, {
            runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21aa'] },
          })
          return result.violations.map((violation) => ({
            id: violation.id,
            nodes: violation.nodes.length,
            sample: violation.nodes
              .slice(0, 3)
              .map((node) => node.failureSummary ?? node.target.join(' ')),
            targets: violation.nodes.slice(0, 3).map((node) => node.target.join(' ')),
          }))
        })
        for (const violation of violations) {
          problems.push({ page: `${entry.name}-${scheme}-${viewport.name}`, ...violation })
        }
        if (failures.length > 0) {
          console.warn(`${entry.name} (${scheme}, ${viewport.name}):\n  ${failures.join('\n  ')}`)
        }
        await page.close()
      }
      await context.close()
    }
  }
} finally {
  await browser.close()
  await new Promise((done) => server.httpServer.close(done))
}
for (const problem of problems) {
  console.warn(
    `${problem.page}: ${problem.id} on ${problem.nodes} elements\n    ${problem.targets.join('\n    ')}\n    ${problem.sample[0]?.replaceAll('\n', ' ') ?? ''}`,
  )
}
console.log(`${written.length} screenshots in ${output}, ${problems.length} accessibility findings`)
if (problems.length > 0) {
  process.exitCode = 1
}
