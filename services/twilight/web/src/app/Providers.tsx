import { localStorageColorSchemeManager, MantineProvider } from '@mantine/core'
import { Notifications } from '@mantine/notifications'
import type { QueryClient } from '@tanstack/react-query'
import { QueryClientProvider } from '@tanstack/react-query'
import type { ReactNode } from 'react'
import { cssVariablesResolver, theme } from './theme'

const colorSchemeManager = localStorageColorSchemeManager({ key: 'twilight.color-scheme' })

export function Providers({
  client,
  children,
  test = false,
}: {
  client: QueryClient
  children: ReactNode
  test?: boolean
}) {
  return (
    <MantineProvider
      theme={theme}
      cssVariablesResolver={cssVariablesResolver}
      defaultColorScheme="dark"
      colorSchemeManager={colorSchemeManager}
      env={test ? 'test' : 'default'}
    >
      <Notifications position="bottom-right" limit={4} />
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    </MantineProvider>
  )
}
