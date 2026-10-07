import {
  Group,
  Input,
  Radio,
  Select,
  SimpleGrid,
  Stack,
  Switch,
  TagsInput,
  Text,
  TextInput,
} from '@mantine/core'
import type { Icon } from '@tabler/icons-react'
import { IconLock, IconPackage, IconSettingsCheck, IconTerminal2 } from '@tabler/icons-react'
import { useId } from 'react'
import type { Action, ActionKind, LogLevel } from '../../api/types'
import { CodeEditor } from '../../editors/CodeEditor'
import { shellExtension } from '../../editors/shell'
import type { Issue } from '../validation'
import { issueAt, issuesUnder, maximumCollectedFiles } from '../validation'
import { DurationInput } from './DurationInput'
import classes from './CampaignEditor.module.css'

const kinds: { kind: ActionKind; label: string; icon: Icon; description: string }[] = [
  {
    kind: 'run_script',
    label: 'Run script',
    icon: IconTerminal2,
    description:
      'Runs a dusk shell script once on each node. A node that never reports back is shown to you, not run again.',
  },
  {
    kind: 'ensure_version',
    label: 'Ensure version',
    icon: IconPackage,
    description:
      'Brings each node to a version and keeps it there: the script runs where a node reports anything else, and again where it drifts later.',
  },
  {
    kind: 'ensure_config',
    label: 'Ensure config',
    icon: IconSettingsCheck,
    description: 'Brings each node to a config hash and keeps it there, the same way as a version.',
  },
  {
    kind: 'quarantine',
    label: 'Quarantine',
    icon: IconLock,
    description:
      'Moves nodes to quarantine. They stay connected, but callers may only do what the quarantine policy allows.',
  },
]

const logLevels: LogLevel[] = ['error', 'warn', 'info', 'debug', 'trace']

interface ScriptFieldProperties {
  value: string
  onChange: (script: string) => void
  description: string
  error: string | undefined
  required: boolean
}

function ScriptField({ value, onChange, description, error, required }: ScriptFieldProperties) {
  const fieldId = useId()
  return (
    <Input.Wrapper
      label="Script"
      description={description}
      error={error}
      required={required}
      id={fieldId}
      labelProps={{ htmlFor: undefined, id: `${fieldId}-label` }}
    >
      <div className={classes.script}>
        <CodeEditor
          value={value}
          onChange={onChange}
          label="Script"
          language={shellExtension}
          placeholder={'kvs set app.channel stable\necho done'}
          minimumRows={6}
          invalid={error !== undefined}
          describedBy={`${fieldId}-description${error === undefined ? '' : ` ${fieldId}-error`}`}
        />
      </div>
    </Input.Wrapper>
  )
}

interface ActionEditorProperties {
  action: Action
  onKindChange: (kind: ActionKind) => void
  onChange: (action: Action) => void
  issues: Issue[]
}

export function ActionEditor({ action, onKindChange, onChange, issues }: ActionEditorProperties) {
  const scriptError = issueAt(issues, 'action.script')
  const kindId = useId()
  return (
    <Stack gap="lg">
      <Radio.Group
        value={action.kind}
        onChange={(value) => onKindChange(value as ActionKind)}
        label="What the nodes do"
        labelProps={{ className: classes.groupLabel }}
      >
        <SimpleGrid cols={{ base: 1, sm: 2, xl: 4 }} spacing="sm" mt={6}>
          {kinds.map((entry) => {
            const KindIcon = entry.icon
            return (
              <Radio.Card
                key={entry.kind}
                value={entry.kind}
                className={classes.kindCard}
                aria-label={entry.label}
                aria-describedby={`${kindId}-${entry.kind}`}
              >
                <Group gap="sm" wrap="nowrap" align="flex-start">
                  <Radio.Indicator mt={2} />
                  <div>
                    <Group gap={6} wrap="nowrap">
                      <KindIcon size={16} aria-hidden="true" className={classes.kindIcon} />
                      <Text fw={650} size="sm">
                        {entry.label}
                      </Text>
                    </Group>
                    <Text id={`${kindId}-${entry.kind}`} size="xs" c="dimmed" mt={4}>
                      {entry.description}
                    </Text>
                  </div>
                </Group>
              </Radio.Card>
            )
          })}
        </SimpleGrid>
      </Radio.Group>
      {action.kind === 'ensure_version' && (
        <SimpleGrid cols={{ base: 1, sm: 2 }} spacing="md">
          <TextInput
            label="Version"
            description="What the nodes must report once the script has run."
            placeholder="0.2.2"
            required
            value={action.version}
            onChange={(event) => onChange({ ...action, version: event.currentTarget.value })}
            error={issueAt(issues, 'action.version')}
            classNames={{ input: 'mono' }}
          />
          <TextInput
            label="Read from"
            description="The key a node reports its version under, read again after the script."
            required
            value={action.version_key}
            onChange={(event) => onChange({ ...action, version_key: event.currentTarget.value })}
            error={issueAt(issues, 'action.version_key')}
            classNames={{ input: 'mono' }}
          />
        </SimpleGrid>
      )}
      {action.kind === 'ensure_config' && (
        <TextInput
          label="Config hash"
          description="What the nodes must report under dusk.config.hash once the script has run."
          placeholder="b41d9a0c3e5f…"
          required
          value={action.config_hash}
          onChange={(event) => onChange({ ...action, config_hash: event.currentTarget.value })}
          error={issueAt(issues, 'action.config_hash')}
          classNames={{ input: 'mono' }}
        />
      )}
      <ScriptField
        value={action.script ?? ''}
        onChange={(script) => onChange({ ...action, script })}
        required={action.kind !== 'quarantine'}
        error={scriptError}
        description={
          action.kind === 'run_script'
            ? 'Dusk shell. Each attempt runs at its own pid, so a node runs it at most once per attempt, even if it is sent twice.'
            : action.kind === 'quarantine'
              ? 'Optional. Runs first on nodes that are online, for example to lock a kiosk.'
              : 'Dusk shell. It runs only where the node reports something else, and is retried up to the attempts set under Timeouts and retries; a node that drifts later gets it again.'
        }
      />
      {action.kind === 'quarantine' && (
        <Switch
          label="Quarantine only the nodes where the script succeeds"
          description="Off: every node in an open phase is quarantined, whether the script ran or not."
          checked={action.require_script_success}
          onChange={(event) =>
            onChange({ ...action, require_script_success: event.currentTarget.checked })
          }
          error={issueAt(issues, 'action.require_script_success')}
        />
      )}
      {action.kind === 'run_script' && (
        <Stack gap="md">
          <TagsInput
            label="Collect files"
            description={`Absolute paths on the node, uploaded after the script, at most ${maximumCollectedFiles}. Press Enter after each.`}
            placeholder={(action.collect_files ?? []).length === 0 ? '/var/log/app/crash.log' : ''}
            value={action.collect_files ?? []}
            onChange={(files) => onChange({ ...action, collect_files: files })}
            maxTags={maximumCollectedFiles}
            splitChars={[',']}
            clearable
            error={issuesUnder(issues, 'action.collect_files')[0]?.message}
            classNames={{ pill: 'mono' }}
          />
          <Switch
            label="Stream logs while it runs"
            description="Sends each node's logs at this level and above to the collector."
            checked={action.stream_logs !== null && action.stream_logs !== undefined}
            onChange={(event) =>
              onChange({
                ...action,
                stream_logs: event.currentTarget.checked
                  ? { level: 'info', duration_seconds: 900 }
                  : null,
              })
            }
          />
          {action.stream_logs !== null && action.stream_logs !== undefined && (
            <SimpleGrid cols={{ base: 1, sm: 2 }} spacing="md" className={classes.indented}>
              <Select
                label="Level"
                data={logLevels}
                value={action.stream_logs.level}
                onChange={(level) =>
                  action.stream_logs &&
                  onChange({
                    ...action,
                    stream_logs: { ...action.stream_logs, level: (level ?? 'info') as LogLevel },
                  })
                }
              />
              <DurationInput
                label="For"
                value={action.stream_logs.duration_seconds}
                onChange={(seconds) =>
                  action.stream_logs &&
                  onChange({
                    ...action,
                    stream_logs: { ...action.stream_logs, duration_seconds: seconds },
                  })
                }
                error={issueAt(issues, 'action.stream_logs.duration_seconds')}
              />
            </SimpleGrid>
          )}
        </Stack>
      )}
    </Stack>
  )
}
