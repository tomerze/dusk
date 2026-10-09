import '@fontsource-variable/atkinson-hyperlegible-mono'
import '@fontsource-variable/atkinson-hyperlegible-next'
import '@mantine/core/styles.css'
import '@mantine/notifications/styles.css'
import './app/global.css'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { createBrowserRouter, RouterProvider } from 'react-router'
import { Providers } from './app/Providers'
import { createQueryClient } from './app/queryClient'
import { routes } from './app/routes'

async function start() {
  if (import.meta.env.MODE === 'mock') {
    const { startMocks } = await import('./mocks/browser')
    await startMocks()
  }
  const root = document.getElementById('root')
  if (root === null) {
    throw new Error('index.html has no #root element')
  }
  createRoot(root).render(
    <StrictMode>
      <Providers client={createQueryClient()}>
        <RouterProvider router={createBrowserRouter(routes)} />
      </Providers>
    </StrictMode>,
  )
}

void start()
