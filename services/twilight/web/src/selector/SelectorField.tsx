import { Group, Input, Loader, Stack, Text } from '@mantine/core'
import { IconAlertTriangle, IconCircleCheck, IconCircleX } from '@tabler/icons-react'
import { useEffect, useId, useMemo, useRef } from 'react'
import { errorMessage } from '../api/client'
import type { SelectorValidation } from '../api/types'
import { StatusPill } from '../components/StatusPill'
import { lifecycleStyles } from '../components/status'
import { CodeEditor } from '../editors/CodeEditor'
import { selectorDiagnostics, selectorExtension } from '../editors/selector'
import { formatNumber } from '../format'
import classes from './SelectorField.module.css'
import { useSelectorValidation } from './useSelectorValidation'

interface SelectorFieldProperties {
  value: string
  onChange: (value: string) => void
  label?: string
  description?: string
  showSample?: boolean
  singleLine?: boolean
  onSubmit?: () => void
  onValidation?: (validation: SelectorValidation | undefined) => void
}

function Sample({ validation }: { validation: SelectorValidation }) {
  const headingId = useId()
  if (validation.sample.length === 0) {
    return null
  }
  return (
    <div className={classes.sample} tabIndex={0} role="region" aria-labelledby={headingId}>
      <Text size="xs" c="dimmed" mb={4} id={headingId}>
        A sample of the matching nodes
      </Text>
      <table className={classes.sampleTable}>
        <thead>
          <tr>
            <th scope="col">Hostname</th>
            <th scope="col">OS</th>
            <th scope="col">Dusk</th>
            <th scope="col">Country</th>
            <th scope="col">Lifecycle</th>
          </tr>
        </thead>
        <tbody>
          {validation.sample.map((node) => (
            <tr key={`${node.device_id}/${node.installation_id}`}>
              <td>
                <span className={classes.online} data-online={node.online} aria-hidden="true" />
                <Text component="span" size="xs" className="mono">
                  {node.hostname ?? node.device_id}
                </Text>
              </td>
              <td>
                <Text component="span" size="xs">
                  {[node.os_name, node.os_version].filter(Boolean).join(' ')}
                </Text>
              </td>
              <td>
                <Text component="span" size="xs" className="mono">
                  {node.dusk_version ?? 'unknown'}
                </Text>
              </td>
              <td>
                <Text component="span" size="xs">
                  {node.country ?? 'unknown'}
                </Text>
              </td>
              <td>
                <StatusPill status={lifecycleStyles[node.lifecycle]} size="xs" />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

export function SelectorField({
  value,
  onChange,
  label = 'Selector',
  description,
  showSample = true,
  singleLine = false,
  onSubmit,
  onValidation,
}: SelectorFieldProperties) {
  const { validation, checking, stale, failure, validatedSelector } = useSelectorValidation(value)
  const statusId = useId()
  const current = !stale && validatedSelector === value ? validation : undefined
  const report = useRef(onValidation)
  useEffect(() => {
    report.current = onValidation
  })
  useEffect(() => {
    report.current?.(current)
  }, [current])
  const diagnostics = useMemo(
    () => selectorDiagnostics(value, current?.ok === false ? current.error : null),
    [current, value],
  )
  const invalid = current?.ok === false

  let status
  if (failure !== null && failure !== undefined) {
    status = (
      <Group gap={6} wrap="nowrap" className={classes.status} data-tone="warning">
        <IconAlertTriangle size={16} aria-hidden="true" />
        <Text size="sm">Could not check the selector: {errorMessage(failure)}</Text>
      </Group>
    )
  } else if (current === undefined) {
    status = (
      <Group gap={8} wrap="nowrap" className={classes.status}>
        <Loader size={14} />
        <Text size="sm" c="dimmed">
          Checking the selector
        </Text>
      </Group>
    )
  } else if (!current.ok && current.error !== null) {
    status = (
      <Group gap={6} wrap="nowrap" align="flex-start" className={classes.status} data-tone="danger">
        <IconCircleX size={16} aria-hidden="true" className={classes.statusIcon} />
        <Text size="sm">
          {current.error.message}
          <Text component="span" size="xs" c="dimmed">
            {' '}
            (at character {formatNumber(current.error.position + 1)})
          </Text>
        </Text>
      </Group>
    )
  } else {
    status = (
      <Group gap={6} wrap="wrap" className={classes.status} data-tone="ok">
        <IconCircleCheck size={16} aria-hidden="true" />
        <Text size="sm" fw={600} className="tabular">
          {current.matched === 1
            ? '1 node matches'
            : `${formatNumber(current.matched)} nodes match`}
        </Text>
        {value.trim().length === 0 && (
          <Text size="sm" c="dimmed">
            An empty selector matches every node.
          </Text>
        )}
        {checking && <Loader size={12} />}
      </Group>
    )
  }

  return (
    <Input.Wrapper label={label} description={description} className={classes.field}>
      <Stack gap={6} mt={description === undefined ? 4 : 6}>
        <CodeEditor
          value={value}
          onChange={onChange}
          label={label}
          language={selectorExtension}
          placeholder={
            singleLine
              ? 'country == "US" and os_name == "debian"'
              : 'country == "US" and os_name in ["debian", "ubuntu"]'
          }
          diagnostics={diagnostics}
          invalid={invalid}
          singleLine={singleLine}
          onSubmit={onSubmit}
          minimumRows={singleLine ? 1 : 2}
          describedBy={statusId}
        />
        <div id={statusId} aria-live="polite">
          {status}
        </div>
        {showSample && current?.ok === true && <Sample validation={current} />}
      </Stack>
    </Input.Wrapper>
  )
}
