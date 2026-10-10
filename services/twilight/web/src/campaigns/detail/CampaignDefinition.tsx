import { Button, CopyButton, DataList, Group, SimpleGrid, Stack, Text, Title } from '@mantine/core'
import { IconCheck, IconCopy } from '@tabler/icons-react'
import type { ReactNode } from 'react'
import type { Action, CampaignDefinition } from '../../api/types'
import { MonoId } from '../../components/MonoId'
import { Section } from '../../components/Section'
import { CodeEditor } from '../../editors/CodeEditor'
import { selectorExtension } from '../../editors/selector'
import { shellExtension } from '../../editors/shell'
import {
  formatAbsolute,
  formatDuration,
  formatNumber,
  formatPercent,
  joinWords,
} from '../../format'
import { actionLabels, definitionOf } from '../display'
import { breakdownLabels, summarizeCampaign } from '../summary'
import classes from './CampaignDefinition.module.css'

function Facts({ items }: { items: [string, ReactNode][] }) {
  return (
    <DataList className={classes.facts}>
      {items.map(([label, value]) => (
        <DataList.Item key={label}>
          <DataList.ItemLabel>{label}</DataList.ItemLabel>
          <DataList.ItemValue>{value}</DataList.ItemValue>
        </DataList.Item>
      ))}
    </DataList>
  )
}

function ActionFacts({ action }: { action: Action }) {
  const items: [string, ReactNode][] = [['Kind', actionLabels[action.kind]]]
  switch (action.kind) {
    case 'run_script':
      items.push(
        [
          'Collects',
          (action.collect_files ?? []).length === 0 ? (
            'no files'
          ) : (
            <Stack gap={2}>
              {(action.collect_files ?? []).map((file) => (
                <Text key={file} size="sm" className="mono">
                  {file}
                </Text>
              ))}
            </Stack>
          ),
        ],
        [
          'Streams logs',
          action.stream_logs === null || action.stream_logs === undefined
            ? 'no'
            : `${action.stream_logs.level} and above for ${formatDuration(action.stream_logs.duration_seconds)}`,
        ],
      )
      break
    case 'ensure_version':
      items.push(
        ['Version', <span className="mono">{action.version}</span>],
        ['Read from', <span className="mono">{action.version_key}</span>],
      )
      break
    case 'ensure_config':
      items.push([
        'Config hash',
        <MonoId value={action.config_hash} label="config hash" maximum={24} />,
      ])
      break
    case 'quarantine':
      items.push([
        'Script must succeed',
        action.require_script_success ? 'yes, or the node is not quarantined' : 'no',
      ])
      break
  }
  return <Facts items={items} />
}

export function CampaignDefinitionView({
  definition: source,
  matched,
}: {
  definition: CampaignDefinition
  matched: number | null
}) {
  const definition = definitionOf(source)
  const summary = summarizeCampaign(definition, matched)
  const { policy, action, selector } = definition
  const script = action.script ?? ''
  return (
    <Stack gap="lg">
      <div className={classes.summary}>
        <Text fw={600} className={classes.headline}>
          {summary.headline}
        </Text>
        <ul className={classes.details}>
          {summary.details.map((detail) => (
            <li key={detail}>
              <Text size="sm" c="dimmed">
                {detail}
              </Text>
            </li>
          ))}
        </ul>
        <Group gap="xs" mt="sm">
          <CopyButton value={JSON.stringify(definition, null, 2)} timeout={2000}>
            {({ copied, copy }) => (
              <Button
                size="xs"
                variant="default"
                onClick={copy}
                leftSection={
                  copied ? (
                    <IconCheck size={14} aria-hidden="true" />
                  ) : (
                    <IconCopy size={14} aria-hidden="true" />
                  )
                }
              >
                {copied ? 'Copied the definition' : 'Copy as JSON'}
              </Button>
            )}
          </CopyButton>
        </Group>
      </div>
      <Section
        title="Selector"
        description={`Matches ${matched === null ? 'an unknown number of' : formatNumber(matched)} nodes now.`}
      >
        {selector.trim().length === 0 ? (
          <Text size="sm">Empty: every node in the inventory.</Text>
        ) : (
          <CodeEditor value={selector} label="Selector" language={selectorExtension} readOnly />
        )}
      </Section>
      <Section title="Action">
        <Stack gap="md">
          <ActionFacts action={action} />
          {script.trim().length > 0 && (
            <div>
              <Title order={3} size="h4" mb={6}>
                Script
              </Title>
              <CodeEditor value={script} label="Script" language={shellExtension} readOnly />
            </div>
          )}
        </Stack>
      </Section>
      <Section title="Policy">
        <SimpleGrid cols={{ base: 1, md: 2 }} spacing="xl">
          <div>
            <Title order={3} size="h4" mb={8}>
              Phases
            </Title>
            <table className={classes.phases}>
              <thead>
                <tr>
                  <th scope="col">Phase</th>
                  <th scope="col">Nodes</th>
                  <th scope="col">Bake</th>
                </tr>
              </thead>
              <tbody>
                {policy.phases.map((phase, index) => (
                  <tr key={`${phase.name}-${index}`}>
                    <td>
                      {index + 1}. {phase.name}
                    </td>
                    <td className="tabular">{phase.percent}%</td>
                    <td className="tabular">{formatDuration(phase.bake_seconds)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <Facts
            items={[
              [
                'Rate',
                `${formatNumber(policy.rate.per_second)} nodes/s, bursts of ${formatNumber(policy.rate.burst)}`,
              ],
              [
                'Sample',
                `${formatNumber(policy.gates.min_sample)} results and nodes past the silent window`,
              ],
              ['Failure limit', formatPercent(policy.gates.max_failure_rate)],
              [
                'Silent limit',
                `${formatPercent(policy.gates.max_silent_rate)} within ${formatDuration(policy.gates.silent_window_seconds)}`,
              ],
              [
                'Broken down by',
                policy.gates.breakdown.length === 0
                  ? 'nothing'
                  : joinWords(
                      policy.gates.breakdown.map((field) => breakdownLabels[field]),
                      'and',
                    ),
              ],
              ['On a failing gate', policy.abort.on_gate_failure === 'pause' ? 'pause' : 'abort'],
              [
                'Failure cap',
                policy.abort.max_total_failures === null
                  ? 'none'
                  : formatNumber(policy.abort.max_total_failures),
              ],
              ['Node timeout', formatDuration(policy.node_timeout_seconds)],
              [
                'Attempts',
                policy.retry.max_attempts === 1
                  ? '1, no retries'
                  : `${policy.retry.max_attempts}, backing off ${formatDuration(policy.retry.initial_backoff_seconds)} to ${formatDuration(policy.retry.max_backoff_seconds)}, ×${policy.retry.multiplier}`,
              ],
              ['Deadline', policy.deadline === null ? 'none' : formatAbsolute(policy.deadline)],
              ['Overlap', policy.allow_overlap ? 'allowed' : 'refused'],
            ]}
          />
        </SimpleGrid>
      </Section>
    </Stack>
  )
}
