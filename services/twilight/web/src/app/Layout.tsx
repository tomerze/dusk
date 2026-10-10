import {
  ActionIcon,
  AppShell,
  Badge,
  Burger,
  Group,
  Stack,
  Text,
  Tooltip,
  UnstyledButton,
  VisuallyHidden,
  useComputedColorScheme,
  useMantineColorScheme,
} from '@mantine/core'
import { useDisclosure } from '@mantine/hooks'
import { notifications } from '@mantine/notifications'
import type { Icon } from '@tabler/icons-react'
import {
  IconAlertTriangle,
  IconLayoutDashboard,
  IconLogout,
  IconMoon,
  IconRocket,
  IconServer2,
  IconSun,
} from '@tabler/icons-react'
import { useQueryClient } from '@tanstack/react-query'
import { NavLink, Outlet, useLocation } from 'react-router'
import { ApiError, apiRequest, errorMessage, storeToken } from '../api/client'
import type { LiveState } from '../api/live'
import { useLiveStream } from '../api/live'
import { useOverview } from '../api/queries'
import type { Me } from '../api/types'
import { Brand } from './Brand'
import { CriticalBanner } from './CriticalBanner'
import classes from './Layout.module.css'

interface NavigationItem {
  to: string
  label: string
  icon: Icon
  end?: boolean
}

const navigation: NavigationItem[] = [
  { to: '/', label: 'Overview', icon: IconLayoutDashboard, end: true },
  { to: '/campaigns', label: 'Campaigns', icon: IconRocket },
  { to: '/nodes', label: 'Nodes', icon: IconServer2 },
  { to: '/alerts', label: 'Alerts', icon: IconAlertTriangle },
]

const liveLabels: Record<LiveState, { label: string; description: string }> = {
  connecting: { label: 'Connecting', description: 'Opening the live update stream.' },
  live: { label: 'Live', description: 'Counters, presence and alerts update as they change.' },
  reconnecting: {
    label: 'Reconnecting',
    description:
      'Live updates are interrupted; pages still refresh on their own every 15 to 30 seconds.',
  },
}

function ColorSchemeToggle() {
  const { setColorScheme } = useMantineColorScheme()
  const computed = useComputedColorScheme('dark')
  const next = computed === 'dark' ? 'light' : 'dark'
  return (
    <Tooltip label={`Switch to ${next} theme`}>
      <ActionIcon
        variant="subtle"
        color="gray"
        size="lg"
        aria-label={`Switch to ${next} theme`}
        onClick={() => setColorScheme(next)}
      >
        {computed === 'dark' ? (
          <IconSun size={18} aria-hidden="true" />
        ) : (
          <IconMoon size={18} aria-hidden="true" />
        )}
      </ActionIcon>
    </Tooltip>
  )
}

function UserFooter({ me, live }: { me: Me; live: LiveState }) {
  const client = useQueryClient()
  const signOut = async () => {
    if (me.authentication === 'oidc') {
      try {
        await apiRequest('POST', '/api/v1/auth/logout', {})
      } catch (failure) {
        if (!(failure instanceof ApiError) || failure.status !== 401) {
          notifications.show({
            color: 'red',
            title: 'Could not sign out',
            message: errorMessage(failure),
          })
          return
        }
      }
    }
    storeToken(null)
    client.clear()
    window.location.assign('/')
  }
  const liveLabel = liveLabels[live]
  return (
    <Stack gap="xs" className={classes.footer}>
      <Tooltip label={liveLabel.description} position="right">
        <Group gap={8} wrap="nowrap" className={classes.live} data-state={live} role="status">
          <span className={classes.liveDot} aria-hidden="true" />
          <Text size="xs" c="dimmed">
            {liveLabel.label}
            <VisuallyHidden>: {liveLabel.description}</VisuallyHidden>
          </Text>
        </Group>
      </Tooltip>
      <Group justify="space-between" wrap="nowrap" gap="xs">
        <Stack gap={0} style={{ minWidth: 0 }}>
          <Text size="sm" fw={600} truncate="end">
            {me.name ?? me.subject}
          </Text>
          <Text size="xs" c="dimmed">
            {me.role}
            {me.authentication === 'dev' ? ', development session' : ''}
          </Text>
        </Stack>
        <Group gap={2} wrap="nowrap">
          <ColorSchemeToggle />
          {me.authentication !== 'dev' && me.authentication !== 'mtls' && (
            <Tooltip label="Sign out">
              <ActionIcon
                variant="subtle"
                color="gray"
                size="lg"
                aria-label="Sign out"
                onClick={() => void signOut()}
              >
                <IconLogout size={18} aria-hidden="true" />
              </ActionIcon>
            </Tooltip>
          )}
        </Group>
      </Group>
    </Stack>
  )
}

export function Layout({ me }: { me: Me }) {
  const [opened, { toggle, close }] = useDisclosure(false)
  const overview = useOverview()
  const live = useLiveStream(import.meta.env.MODE !== 'test')
  const location = useLocation()
  const openAlerts = Object.values(overview.data?.alerts ?? {}).reduce(
    (sum, count) => sum + count,
    0,
  )
  const criticalOpen = overview.data?.alerts.critical ?? 0

  return (
    <AppShell
      header={{ height: { base: 52, sm: 0 } }}
      navbar={{ width: 224, breakpoint: 'sm', collapsed: { mobile: !opened } }}
      padding={0}
    >
      <AppShell.Header hiddenFrom="sm" className={classes.header}>
        <Group h="100%" px="md" justify="space-between" wrap="nowrap">
          <Group gap="sm" wrap="nowrap">
            <Burger
              opened={opened}
              onClick={toggle}
              size="sm"
              aria-label={opened ? 'Close navigation' : 'Open navigation'}
            />
            <Brand />
          </Group>
          <ColorSchemeToggle />
        </Group>
      </AppShell.Header>
      <AppShell.Navbar className={classes.navbar} aria-label="Main">
        <AppShell.Section className={classes.brand} visibleFrom="sm">
          <Brand />
        </AppShell.Section>
        <AppShell.Section grow component="nav" className={classes.links}>
          {navigation.map((item) => {
            const ItemIcon = item.icon
            const count = item.to === '/alerts' ? openAlerts : 0
            return (
              <UnstyledButton
                key={item.to}
                component={NavLink}
                to={item.to}
                end={item.end}
                className={classes.link}
                onClick={close}
              >
                <ItemIcon size={18} stroke={1.8} aria-hidden="true" />
                <span className={classes.linkLabel}>{item.label}</span>
                {count > 0 && (
                  <Badge
                    size="sm"
                    variant={criticalOpen > 0 ? 'filled' : 'light'}
                    color={criticalOpen > 0 ? 'red' : 'gray'}
                    className={classes.count}
                  >
                    {count}
                    <VisuallyHidden> open</VisuallyHidden>
                  </Badge>
                )}
              </UnstyledButton>
            )
          })}
        </AppShell.Section>
        <AppShell.Section>
          <UserFooter me={me} live={live} />
        </AppShell.Section>
      </AppShell.Navbar>
      <AppShell.Main className={classes.main}>
        <CriticalBanner waiting={overview.data?.alerts_unacknowledged.critical ?? 0} />
        <div className={classes.content} key={location.pathname}>
          <Outlet />
        </div>
      </AppShell.Main>
    </AppShell>
  )
}
