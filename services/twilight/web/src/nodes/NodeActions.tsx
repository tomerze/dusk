import {
  Badge,
  Button,
  Code,
  CopyButton,
  Group,
  Menu,
  Modal,
  Select,
  Stack,
  Text,
  TextInput,
  Tooltip,
} from '@mantine/core'
import { notifications } from '@mantine/notifications'
import {
  IconArchive,
  IconCheck,
  IconChevronDown,
  IconCircleCheck,
  IconCopy,
  IconEye,
  IconFileDownload,
  IconLock,
  IconLockOpen,
  IconLogs,
  IconShieldX,
  IconTerminal2,
} from '@tabler/icons-react'
import type { ReactNode } from 'react'
import { useState } from 'react'
import { operatorOnly, usePermissions } from '../api/permissions'
import {
  useCollectFile,
  useDeviceLifecycleChange,
  useLifecycleChange,
  useOpenSession,
  useStreamLogs,
} from '../api/queries'
import type {
  DeviceLifecycle,
  InteractiveSession,
  Lifecycle,
  LogLevel,
  NodeDetail,
} from '../api/types'
import { DurationInput } from '../campaigns/editor/DurationInput'
import { ConfirmDialog } from '../components/ConfirmDialog'
import { MonoId } from '../components/MonoId'
import { formatDuration } from '../format'

type Dialog =
  | 'quarantine'
  | 'release'
  | 'retire'
  | 'revoke'
  | 'retire-device'
  | 'revoke-device'
  | 'lift-device'
  | 'session'
  | 'logs'
  | 'file'

type LifecycleSpecification = {
  title: string
  confirmLabel: string
  done: string
  color: string
  typed: boolean
  body: ReactNode
} & (
  | { device: false; target: Exclude<Lifecycle, 'enrolled'> }
  | { device: true; target: DeviceLifecycle['lifecycle'] }
)

function lifecycleSpecification(dialog: Dialog, name: string): LifecycleSpecification | null {
  switch (dialog) {
    case 'quarantine':
      return {
        target: 'quarantined',
        device: false,
        title: `Quarantine ${name}`,
        confirmLabel: 'Quarantine',
        done: `${name} is quarantined`,
        color: 'yellow',
        typed: false,
        body: (
          <Text size="sm">
            The node stays connected. nightfall drops every client session on it, and from then on a
            caller may only do what both its own role and the quarantine policy allow; a role with
            the quarantine override keeps its access, and each such call raises an alert. Campaigns
            that still target the node may have their dispatches denied there, which pauses them.
            You can release it later.
          </Text>
        ),
      }
    case 'release':
      return {
        target: 'active',
        device: false,
        title: `Release ${name} from quarantine`,
        confirmLabel: 'Release',
        done: `${name} is active again`,
        color: 'dusk',
        typed: false,
        body: (
          <Text size="sm">
            The node becomes active: campaigns and operators reach it again from its next call.
          </Text>
        ),
      }
    case 'retire':
      return {
        target: 'retired',
        device: false,
        title: `Retire ${name}`,
        confirmLabel: 'Retire',
        done: `${name} is retired`,
        color: 'orange',
        typed: false,
        body: (
          <Text size="sm">
            nightfall drops its connection now and refuses this installation at every handshake. The
            node stays in the inventory with its history. Use this for a machine that left the
            fleet.
          </Text>
        ),
      }
    case 'revoke':
      return {
        target: 'revoked',
        device: false,
        title: `Revoke ${name}`,
        confirmLabel: 'Revoke',
        done: `${name} is revoked`,
        color: 'red',
        typed: true,
        body: (
          <Stack gap="xs">
            <Text size="sm">
              nightfall drops its connection now and refuses this installation at every handshake
              while it is revoked.
            </Text>
            <Text size="sm" fw={600}>
              Treat this as final: the UI offers no way to make the installation active again.
            </Text>
          </Stack>
        ),
      }
    case 'retire-device':
      return {
        target: 'retired',
        device: true,
        title: `Retire the device ${name} runs on`,
        confirmLabel: 'Retire the device',
        done: `The device ${name} runs on is retired`,
        color: 'orange',
        typed: true,
        body: (
          <Stack gap="xs">
            <Text size="sm">
              nightfall drops every installation of this machine that is connected now and refuses
              all of them at every handshake, including installations made later: reinstalling dusk
              on the machine does not bring it back. Use this for a machine that left the fleet.
            </Text>
            <Text size="sm">
              Lifting the device block later lets them back in, each with its own lifecycle.
            </Text>
          </Stack>
        ),
      }
    case 'revoke-device':
      return {
        target: 'revoked',
        device: true,
        title: `Revoke the device ${name} runs on`,
        confirmLabel: 'Revoke the device',
        done: `The device ${name} runs on is revoked`,
        color: 'red',
        typed: true,
        body: (
          <Stack gap="xs">
            <Text size="sm">
              nightfall drops every installation of this machine that is connected now and refuses
              all of them at every handshake, including installations made later: reinstalling dusk
              on the machine does not bring it back. Use this for a machine you no longer trust.
            </Text>
            <Text size="sm">
              Lifting the device block later lets them back in, each with its own lifecycle.
            </Text>
          </Stack>
        ),
      }
    case 'lift-device':
      return {
        target: 'active',
        device: true,
        title: `Lift the block on the device ${name} runs on`,
        confirmLabel: 'Lift the block',
        done: `The device ${name} runs on is no longer blocked`,
        color: 'dusk',
        typed: false,
        body: (
          <Text size="sm">
            nightfall stops refusing this machine&apos;s installations because of the device. Each
            installation keeps its own lifecycle: one that was retired or revoked by itself stays
            refused.
          </Text>
        ),
      }
    default:
      return null
  }
}

const sessionLifetimes = [
  { value: '900', label: '15 minutes' },
  { value: '3600', label: '1 hour' },
  { value: '14400', label: '4 hours' },
]

const logLevels: LogLevel[] = ['error', 'warn', 'info', 'debug', 'trace']

function SessionReady({
  session,
  lifetime,
  onClose,
}: {
  session: InteractiveSession | null
  lifetime: number
  onClose: () => void
}) {
  const body =
    session === null ? '' : JSON.stringify({ node: session.node, pid: session.pid }, null, 2)
  return (
    <Modal opened={session !== null} onClose={onClose} title="Session ready" size="lg">
      {session !== null && (
        <Stack gap="md">
          <Text size="sm">
            Send this body to dawn&apos;s <Code>POST /v1/connect</Code> with your own token, then
            use dawn&apos;s shell endpoints. dawn starts a shell at this pid on the node, and every
            call it makes there is ledgered under the pid. Calls under it more than{' '}
            {formatDuration(lifetime)} from now raise an alert.
          </Text>
          <Group gap="xs">
            <Text size="sm" c="dimmed">
              Pid
            </Text>
            <MonoId value={session.pid} label="pid" maximum={20} />
          </Group>
          <Code block className="mono" style={{ maxHeight: 260, overflow: 'auto' }}>
            {body}
          </Code>
          <Group justify="flex-end" gap="sm">
            <CopyButton value={body} timeout={2000}>
              {({ copied, copy }) => (
                <Button
                  variant="default"
                  onClick={copy}
                  leftSection={
                    copied ? (
                      <IconCheck size={16} aria-hidden="true" />
                    ) : (
                      <IconCopy size={16} aria-hidden="true" />
                    )
                  }
                >
                  {copied ? 'Copied' : 'Copy the request body'}
                </Button>
              )}
            </CopyButton>
            <Button onClick={onClose}>Done</Button>
          </Group>
        </Stack>
      )}
    </Modal>
  )
}

export function NodeActions({ node }: { node: NodeDetail }) {
  const { canOperate, canAdminister } = usePermissions()
  const [dialog, setDialog] = useState<Dialog | null>(null)
  const [lifetime, setLifetime] = useState('900')
  const [level, setLevel] = useState<LogLevel>('info')
  const [duration, setDuration] = useState(900)
  const [path, setPath] = useState('')
  const [session, setSession] = useState<InteractiveSession | null>(null)
  const lifecycle = useLifecycleChange(node.device_id, node.installation_id)
  const deviceLifecycle = useDeviceLifecycleChange(node.device_id)
  const openSession = useOpenSession(node.device_id, node.installation_id)
  const streamLogs = useStreamLogs(node.device_id, node.installation_id)
  const collectFile = useCollectFile(node.device_id, node.installation_id)
  const name = node.hostname ?? node.device_id
  const specification = dialog === null ? null : lifecycleSpecification(dialog, name)

  if (!canOperate) {
    return (
      <Tooltip label={operatorOnly}>
        <Badge
          variant="light"
          color="gray"
          size="lg"
          leftSection={<IconEye size={14} aria-hidden="true" />}
          tabIndex={0}
        >
          View only
        </Badge>
      </Tooltip>
    )
  }

  const offline = !node.online
  const deviceBlocked = node.device !== null && node.device.lifecycle !== 'active'
  const reachable = node.lifecycle === 'active' || node.lifecycle === 'enrolled'
  const nodeAction = (label: string, icon: ReactNode, target: Dialog) => {
    const disabledReason = offline
      ? 'The node is offline. This needs a connected node.'
      : !reachable
        ? `The node is ${node.lifecycle}; only an active node takes this.`
        : null
    const button = (
      <Button
        variant="default"
        leftSection={icon}
        disabled={disabledReason !== null}
        onClick={() => setDialog(target)}
      >
        {label}
      </Button>
    )
    return disabledReason === null ? (
      button
    ) : (
      <Tooltip label={disabledReason}>
        <span tabIndex={0}>{button}</span>
      </Tooltip>
    )
  }
  const pathValid = path.startsWith('/') || /^[A-Za-z]:[\\/]/.test(path)
  const durationValid = Number.isInteger(duration) && duration >= 1 && duration <= 86400

  return (
    <Group gap="xs" wrap="wrap">
      {nodeAction('Open session', <IconTerminal2 size={16} aria-hidden="true" />, 'session')}
      {nodeAction('Stream logs', <IconLogs size={16} aria-hidden="true" />, 'logs')}
      {nodeAction('Collect file', <IconFileDownload size={16} aria-hidden="true" />, 'file')}
      <Menu position="bottom-end" withinPortal>
        <Menu.Target>
          <Button variant="default" rightSection={<IconChevronDown size={14} aria-hidden="true" />}>
            Lifecycle
          </Button>
        </Menu.Target>
        <Menu.Dropdown>
          {(node.lifecycle === 'active' || node.lifecycle === 'enrolled') && (
            <Menu.Item
              leftSection={<IconLock size={16} aria-hidden="true" />}
              onClick={() => setDialog('quarantine')}
            >
              Quarantine
            </Menu.Item>
          )}
          {node.lifecycle === 'quarantined' && (
            <Menu.Item
              leftSection={<IconCircleCheck size={16} aria-hidden="true" />}
              onClick={() => setDialog('release')}
            >
              Release from quarantine
            </Menu.Item>
          )}
          {!canAdminister && node.lifecycle !== 'revoked' && (
            <Menu.Label>Retiring and revoking need the admin role</Menu.Label>
          )}
          {node.lifecycle !== 'retired' && node.lifecycle !== 'revoked' && (
            <Menu.Item
              leftSection={<IconArchive size={16} aria-hidden="true" />}
              disabled={!canAdminister}
              onClick={() => setDialog('retire')}
            >
              Retire
            </Menu.Item>
          )}
          {node.lifecycle !== 'revoked' ? (
            <Menu.Item
              color="red"
              leftSection={<IconShieldX size={16} aria-hidden="true" />}
              disabled={!canAdminister}
              onClick={() => setDialog('revoke')}
            >
              Revoke
            </Menu.Item>
          ) : (
            <Menu.Item disabled>The UI does not reactivate a revoked installation</Menu.Item>
          )}
          <Menu.Divider />
          <Menu.Label>Every installation on this machine</Menu.Label>
          {deviceBlocked ? (
            <Menu.Item
              leftSection={<IconLockOpen size={16} aria-hidden="true" />}
              disabled={!canAdminister}
              onClick={() => setDialog('lift-device')}
            >
              Lift the device block
            </Menu.Item>
          ) : (
            <>
              <Menu.Item
                leftSection={<IconArchive size={16} aria-hidden="true" />}
                disabled={!canAdminister}
                onClick={() => setDialog('retire-device')}
              >
                Retire the device
              </Menu.Item>
              <Menu.Item
                color="red"
                leftSection={<IconShieldX size={16} aria-hidden="true" />}
                disabled={!canAdminister}
                onClick={() => setDialog('revoke-device')}
              >
                Revoke the device
              </Menu.Item>
            </>
          )}
        </Menu.Dropdown>
      </Menu>
      {specification !== null && (
        <ConfirmDialog
          opened
          onClose={() => setDialog(null)}
          title={specification.title}
          confirmLabel={specification.confirmLabel}
          color={specification.color}
          reason="required"
          reasonLabel="Reason"
          reasonDescription={
            !specification.device
              ? "Kept as the node's lifecycle reason and sent with the node-state change."
              : specification.target === 'active'
                ? "Kept as the device's lifecycle reason."
                : "Kept as the device's lifecycle reason and sent with the node-state change."
          }
          typedConfirmation={specification.typed ? name : undefined}
          consequences={specification.body}
          onConfirm={async (reason) => {
            if (specification.device) {
              await deviceLifecycle.mutateAsync({ lifecycle: specification.target, reason })
            } else {
              await lifecycle.mutateAsync({ lifecycle: specification.target, reason })
            }
            notifications.show({ color: 'green', message: specification.done })
          }}
        />
      )}
      <ConfirmDialog
        opened={dialog === 'session'}
        onClose={() => setDialog(null)}
        title={`Open a session on ${name}`}
        confirmLabel="Open session"
        reason="required"
        reasonLabel="Why do you need a shell on this node?"
        reasonDescription="Written to twilight's log with the pid."
        consequences={
          <Text size="sm">
            twilight picks a pid for this session and records it, with you, as intended for this
            node. Every call dawn makes under it is ledgered and checked against that record; calls
            under it after the time you pick raise an alert.
          </Text>
        }
        onConfirm={async (reason) => {
          const opened = await openSession.mutateAsync({ reason, ttl_seconds: Number(lifetime) })
          setSession(opened)
        }}
      >
        <Select
          label="Valid for"
          data={sessionLifetimes}
          value={lifetime}
          onChange={(value) => setLifetime(value ?? '900')}
        />
      </ConfirmDialog>
      <ConfirmDialog
        opened={dialog === 'logs'}
        onClose={() => setDialog(null)}
        title={`Stream logs from ${name}`}
        confirmLabel="Stream logs"
        confirmDisabled={!durationValid}
        consequences={
          <Text size="sm">
            The node sends its log records at this level and above to the OpenTelemetry collector
            for the time you pick.
          </Text>
        }
        onConfirm={async () => {
          const accepted = await streamLogs.mutateAsync({ level, duration_seconds: duration })
          notifications.show({
            color: 'green',
            title: `Streaming ${level} logs for ${formatDuration(duration)}`,
            message: `Stream ${accepted.stream_id}`,
          })
        }}
      >
        <Group grow align="flex-start">
          <Select
            label="Level"
            data={logLevels}
            value={level}
            onChange={(value) => setLevel((value ?? 'info') as LogLevel)}
          />
          <DurationInput
            label="For"
            value={duration}
            onChange={setDuration}
            error={durationValid ? undefined : 'Stream for 1s to 24h.'}
          />
        </Group>
      </ConfirmDialog>
      <ConfirmDialog
        opened={dialog === 'file'}
        onClose={() => setDialog(null)}
        title={`Collect a file from ${name}`}
        confirmLabel="Collect file"
        confirmDisabled={!pathValid}
        consequences={
          <Text size="sm">
            dawn reads the file from the node and uploads it to the Dusk stack&apos;s object store
            under <Code>files/&lt;device&gt;/&lt;installation&gt;/&lt;pid&gt;/</Code>. A dusk.files
            event records its size and SHA-256.
          </Text>
        }
        onConfirm={async () => {
          const accepted = await collectFile.mutateAsync({ path })
          notifications.show({
            color: 'green',
            title: 'Collecting the file',
            message: `Upload ${accepted.upload_id}`,
          })
        }}
      >
        <TextInput
          label="Path on the node"
          placeholder="/var/log/syslog"
          value={path}
          onChange={(event) => setPath(event.currentTarget.value)}
          error={path.length > 0 && !pathValid ? 'Use an absolute path.' : undefined}
          classNames={{ input: 'mono' }}
          data-autofocus
        />
      </ConfirmDialog>
      <SessionReady
        session={session}
        lifetime={Number(lifetime)}
        onClose={() => setSession(null)}
      />
    </Group>
  )
}
