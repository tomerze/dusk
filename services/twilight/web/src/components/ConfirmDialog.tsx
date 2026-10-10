import { Alert, Button, Code, Group, Modal, Stack, Text, Textarea, TextInput } from '@mantine/core'
import type { ReactNode } from 'react'
import { useId, useState } from 'react'
import { errorMessage } from '../api/client'

export interface ConfirmDialogProperties {
  opened: boolean
  onClose: () => void
  title: string
  consequences: ReactNode
  confirmLabel: string
  color?: string
  reason?: 'required' | 'optional' | 'none'
  reasonLabel?: string
  reasonDescription?: string
  typedConfirmation?: string
  children?: ReactNode
  confirmDisabled?: boolean
  onConfirm: (reason: string) => Promise<unknown>
}

export function ConfirmDialog(properties: ConfirmDialogProperties) {
  return (
    <Modal
      opened={properties.opened}
      onClose={properties.onClose}
      title={properties.title}
      size="lg"
    >
      {properties.opened && <ConfirmBody {...properties} />}
    </Modal>
  )
}

function ConfirmBody({
  onClose,
  consequences,
  confirmLabel,
  color = 'dusk',
  reason = 'none',
  reasonLabel = 'Reason',
  reasonDescription,
  typedConfirmation,
  children,
  confirmDisabled = false,
  onConfirm,
}: ConfirmDialogProperties) {
  const [reasonText, setReasonText] = useState('')
  const [typed, setTyped] = useState('')
  const [pending, setPending] = useState(false)
  const [failure, setFailure] = useState<string | null>(null)
  const typedId = useId()
  const reasonMissing = reason === 'required' && reasonText.trim().length === 0
  const typedMismatch = typedConfirmation !== undefined && typed.trim() !== typedConfirmation
  const blocked = reasonMissing || typedMismatch || confirmDisabled || pending

  const confirm = async () => {
    setPending(true)
    setFailure(null)
    try {
      await onConfirm(reasonText.trim())
      onClose()
    } catch (error) {
      setFailure(errorMessage(error))
    } finally {
      setPending(false)
    }
  }

  return (
    <form
      onSubmit={(event) => {
        event.preventDefault()
        if (!blocked) {
          void confirm()
        }
      }}
    >
      <Stack gap="md">
        <div>{consequences}</div>
        {children}
        {reason !== 'none' && (
          <Textarea
            label={reasonLabel}
            description={reasonDescription}
            required={reason === 'required'}
            autosize
            minRows={2}
            maxRows={6}
            value={reasonText}
            onChange={(event) => setReasonText(event.currentTarget.value)}
            data-autofocus
          />
        )}
        {typedConfirmation !== undefined && (
          <TextInput
            id={typedId}
            label={
              <Text component="span" size="sm">
                Type <Code>{typedConfirmation}</Code> to confirm
              </Text>
            }
            autoComplete="off"
            spellCheck={false}
            value={typed}
            onChange={(event) => setTyped(event.currentTarget.value)}
            styles={{ input: { fontFamily: 'var(--mantine-font-family-monospace)' } }}
          />
        )}
        {failure !== null && (
          <Alert color="red" variant="light" role="alert" title={`${confirmLabel} failed`}>
            {failure}
          </Alert>
        )}
        <Group justify="flex-end" gap="sm">
          <Button variant="default" onClick={onClose} disabled={pending}>
            Cancel
          </Button>
          <Button type="submit" color={color} disabled={blocked} loading={pending}>
            {confirmLabel}
          </Button>
        </Group>
      </Stack>
    </form>
  )
}
