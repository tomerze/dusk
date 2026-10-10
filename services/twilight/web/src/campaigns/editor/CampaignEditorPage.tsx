import {
  Alert,
  Anchor,
  Button,
  Group,
  Modal,
  Skeleton,
  Stack,
  Text,
  Textarea,
  TextInput,
  Title,
} from '@mantine/core'
import { notifications } from '@mantine/notifications'
import {
  IconAlertTriangle,
  IconArrowLeft,
  IconCircleCheck,
  IconDeviceFloppy,
  IconEye,
  IconPlayerPlay,
} from '@tabler/icons-react'
import { useRef, useState } from 'react'
import { Link, useBlocker, useNavigate, useParams, useSearchParams } from 'react-router'
import { ApiError, errorMessage } from '../../api/client'
import { operatorOnly, usePermissions } from '../../api/permissions'
import { useQueryClient } from '@tanstack/react-query'
import {
  queryKeys,
  startCampaign,
  useCampaign,
  useCreateCampaign,
  useUpdateCampaign,
} from '../../api/queries'
import type {
  Action,
  ActionKind,
  Campaign,
  CampaignDefinition,
  SelectorValidation,
} from '../../api/types'
import { NotFound } from '../../app/RouteError'
import { PageHeader } from '../../components/PageHeader'
import { QueryError } from '../../components/QueryError'
import { Section } from '../../components/Section'
import { formatNumber, plural } from '../../format'
import { SelectorField } from '../../selector/SelectorField'
import { defaultAction, defaultPolicy, emptyDefinition } from '../defaults'
import { CampaignDefinitionView } from '../detail/CampaignDefinition'
import { definitionOf } from '../display'
import { summarizeCampaign } from '../summary'
import type { Issue } from '../validation'
import { validateDefinition } from '../validation'
import { useOverlaps } from '../overlaps'
import { ActionEditor } from './ActionEditor'
import { PhaseEditor } from './PhaseEditor'
import { GatesEditor, LimitsEditor, RateEditor } from './PolicyEditor'
import classes from './CampaignEditor.module.css'

const sections = [
  { id: 'campaign-basics', title: 'Campaign', prefixes: ['name', 'description'] },
  { id: 'campaign-nodes', title: 'Nodes', prefixes: ['selector'] },
  { id: 'campaign-action', title: 'Action', prefixes: ['action'] },
  { id: 'campaign-rollout', title: 'Rollout', prefixes: ['policy.phases', 'policy.rate'] },
  { id: 'campaign-gates', title: 'Health gates', prefixes: ['policy.gates', 'policy.abort'] },
  {
    id: 'campaign-limits',
    title: 'Timeouts and retries',
    prefixes: ['policy.node_timeout_seconds', 'policy.retry', 'policy.deadline'],
  },
] as const

function sectionOf(issue: Issue) {
  return (
    sections.find((section) =>
      section.prefixes.some(
        (prefix) => issue.path === prefix || issue.path.startsWith(`${prefix}.`),
      ),
    ) ?? sections[0]
  )
}

function changeKind(definition: CampaignDefinition, kind: ActionKind): CampaignDefinition {
  if (definition.action.kind === kind) {
    return definition
  }
  const action: Action = { ...defaultAction(kind), script: definition.action.script ?? '' }
  const previousDefault = defaultPolicy(definition.action.kind).retry.max_attempts
  const retry =
    definition.policy.retry.max_attempts === previousDefault
      ? { ...definition.policy.retry, max_attempts: defaultPolicy(kind).retry.max_attempts }
      : definition.policy.retry
  return { ...definition, action, policy: { ...definition.policy, retry } }
}

function startSentence(definition: CampaignDefinition, matched: number | null): string {
  const first = definition.policy.phases[0]
  if (first === undefined) {
    return 'Dispatch begins at once.'
  }
  const nodes = matched === null ? null : Math.round((matched * first.percent) / 100)
  return `Starting dispatches to ${first.name} (${first.percent}%${nodes === null ? '' : `, about ${plural(nodes, 'node')}`}) at once.`
}

type Mode = { kind: 'create' } | { kind: 'edit'; campaign: Campaign }

interface CampaignEditorProperties {
  mode: Mode
  initial: CampaignDefinition
}

function CampaignEditor({ mode, initial }: CampaignEditorProperties) {
  const navigate = useNavigate()
  const client = useQueryClient()
  const { canOperate } = usePermissions()
  const [definition, setDefinition] = useState(initial)
  const [validation, setValidation] = useState<SelectorValidation | undefined>(undefined)
  const [view, setView] = useState<'edit' | 'review'>('edit')
  const [showErrors, setShowErrors] = useState(false)
  const [saving, setSaving] = useState<'draft' | 'start' | null>(null)
  const [saveFailure, setSaveFailure] = useState<string | null>(null)
  const leaving = useRef(false)
  const create = useCreateCampaign()
  const update = useUpdateCampaign(mode.kind === 'edit' ? mode.campaign.id : '')
  const dirty = JSON.stringify(definition) !== JSON.stringify(initial)
  const blocker = useBlocker(
    ({ currentLocation, nextLocation }) =>
      dirty && !leaving.current && currentLocation.pathname !== nextLocation.pathname,
  )

  const matched = validation?.ok === true ? validation.matched : null
  const issues: Issue[] = validateDefinition(definition)
  if (validation?.ok === false && validation.error !== null) {
    issues.unshift({ path: 'selector', message: `Fix the selector: ${validation.error.message}` })
  }
  const visibleIssues = showErrors ? issues : []
  const summary = summarizeCampaign(definition, matched)
  const { overlaps } = useOverlaps(
    definition,
    validation?.ok === true,
    mode.kind === 'edit' ? mode.campaign.id : null,
  )
  const ensure =
    definition.action.kind === 'ensure_version' || definition.action.kind === 'ensure_config'
  const overlapBlocksStart = ensure && overlaps.length > 0 && !definition.policy.allow_overlap

  const setPolicy = (policy: CampaignDefinition['policy']) =>
    setDefinition((current) => ({ ...current, policy }))

  const review = () => {
    setShowErrors(true)
    const first = issues[0]
    if (first !== undefined) {
      document.getElementById(sectionOf(first).id)?.scrollIntoView({ block: 'start' })
      return
    }
    setSaveFailure(null)
    setView('review')
    window.scrollTo({ top: 0 })
  }

  const save = async (start: boolean) => {
    setSaving(start ? 'start' : 'draft')
    setSaveFailure(null)
    let saved: Campaign
    try {
      saved =
        mode.kind === 'edit'
          ? await update.mutateAsync({
              ...definitionOf(definition),
              version: mode.campaign.version,
            })
          : await create.mutateAsync(definitionOf(definition))
    } catch (failure) {
      setSaveFailure(errorMessage(failure))
      setSaving(null)
      return
    }
    leaving.current = true
    if (start) {
      try {
        const started = await startCampaign(saved.id)
        client.setQueryData(queryKeys.campaign(started.id), started)
        void client.invalidateQueries({ queryKey: ['campaigns'] })
        void client.invalidateQueries({ queryKey: queryKeys.overview })
        notifications.show({ color: 'green', message: `${saved.name} started` })
      } catch (failure) {
        notifications.show({
          color: 'red',
          title: 'Saved as a draft, but it did not start',
          message: errorMessage(failure),
          autoClose: false,
        })
      }
    } else {
      notifications.show({
        color: 'green',
        message: mode.kind === 'edit' ? 'Draft saved' : `${saved.name} saved as a draft`,
      })
    }
    void navigate(`/campaigns/${saved.id}`, { replace: true })
  }

  const title = mode.kind === 'edit' ? `Edit ${mode.campaign.name}` : 'New campaign'
  const trail =
    mode.kind === 'edit'
      ? [
          { label: 'Campaigns', to: '/campaigns' },
          { label: mode.campaign.name, to: `/campaigns/${mode.campaign.id}` },
        ]
      : [{ label: 'Campaigns', to: '/campaigns' }]

  return (
    <>
      <PageHeader
        trail={trail}
        title={title}
        documentTitle={title}
        description="Pick the nodes, say what they do, and how fast it rolls out. Nothing is sent to a node until the campaign starts."
      />
      {!canOperate && (
        <Alert color="gray" variant="light" icon={<IconEye size={18} aria-hidden="true" />} mb="lg">
          {operatorOnly} You can fill this in to see what a campaign would do, but not save it.
        </Alert>
      )}
      {view === 'edit' ? (
        <div className={classes.layout}>
          <Stack gap="lg" className={classes.form}>
            <Section title="Campaign" id={sections[0].id}>
              <Stack gap="md">
                <TextInput
                  label="Name"
                  required
                  placeholder="Dusk 0.2.2 to retail stores"
                  value={definition.name}
                  onChange={(event) => {
                    const name = event.currentTarget.value
                    setDefinition((current) => ({ ...current, name }))
                  }}
                  error={visibleIssues.find((issue) => issue.path === 'name')?.message}
                  maxLength={200}
                />
                <Textarea
                  label="Description"
                  description="Why this campaign exists. Shown on its page."
                  autosize
                  minRows={2}
                  maxRows={6}
                  value={definition.description}
                  onChange={(event) => {
                    const description = event.currentTarget.value
                    setDefinition((current) => ({ ...current, description }))
                  }}
                />
              </Stack>
            </Section>
            <Section
              title="Nodes"
              id={sections[1].id}
              description="Which nodes it applies to. A node joins when it first matches while its phase is open, and stays."
            >
              <SelectorField
                value={definition.selector}
                onChange={(selector) => setDefinition((current) => ({ ...current, selector }))}
                description='Fields such as country or os_build, facts["key"], and, or, not, in. For every node, use has(device_id).'
                onValidation={setValidation}
              />
              {overlaps.length > 0 && (
                <Alert
                  color="yellow"
                  variant="light"
                  mt="sm"
                  icon={<IconAlertTriangle size={18} aria-hidden="true" />}
                  title="Overlaps a running campaign"
                >
                  <Stack gap={4}>
                    {overlaps.map((overlap) => (
                      <Text size="sm" key={overlap.campaign.id}>
                        {plural(overlap.nodes, 'matching node')}{' '}
                        {overlap.nodes === 1 ? 'is' : 'are'} also targeted by{' '}
                        <Text component="span" fw={600} size="sm">
                          {overlap.campaign.name}
                        </Text>
                        .
                      </Text>
                    ))}
                    <Text size="sm">
                      Only the earliest-started campaign acts on a node; in the others its row
                      becomes a conflict. Starting this campaign needs Allow overlap in the policy.
                    </Text>
                  </Stack>
                </Alert>
              )}
            </Section>
            <Section title="Action" id={sections[2].id}>
              <ActionEditor
                action={definition.action}
                onKindChange={(kind) => setDefinition((current) => changeKind(current, kind))}
                onChange={(action) => setDefinition((current) => ({ ...current, action }))}
                issues={visibleIssues}
              />
            </Section>
            <Section
              title="Rollout"
              id={sections[3].id}
              description="Each phase reaches a larger share of the matched nodes. The next opens once this one has baked and its gate passes. Which phase a node is in never changes, even across a pause."
            >
              <Stack gap="lg">
                <PhaseEditor
                  phases={definition.policy.phases}
                  onChange={(phases) => setPolicy({ ...definition.policy, phases })}
                  silentWindowSeconds={definition.policy.gates.silent_window_seconds}
                  nodeTimeoutSeconds={definition.policy.node_timeout_seconds}
                  matched={matched}
                />
                <RateEditor
                  policy={definition.policy}
                  onChange={setPolicy}
                  issues={visibleIssues}
                />
              </Stack>
            </Section>
            <Section
              title="Health gates"
              id={sections[4].id}
              description="Checked while the campaign runs, over every phase opened so far. A phase only advances while both rates stay under their limits."
            >
              <GatesEditor policy={definition.policy} onChange={setPolicy} issues={visibleIssues} />
            </Section>
            <Section title="Timeouts and retries" id={sections[5].id}>
              <LimitsEditor
                policy={definition.policy}
                onChange={setPolicy}
                issues={visibleIssues}
                kind={definition.action.kind}
              />
            </Section>
          </Stack>
          <aside className={classes.rail} aria-label="Summary">
            <div className={classes.summary}>
              <Text size="xs" c="dimmed" mb={6}>
                Summary
              </Text>
              <Text size="sm" fw={600} className={classes.headline}>
                {summary.headline}
              </Text>
              {matched !== null && (
                <Text size="xs" c="dimmed" mt={8} className="tabular">
                  {plural(matched, 'node')} match now.
                </Text>
              )}
            </div>
            {issues.length > 0 ? (
              <div className={classes.issues} data-shown={showErrors || undefined}>
                <Text size="sm" fw={600}>
                  {issues.length === 1
                    ? '1 thing left before review'
                    : `${formatNumber(issues.length)} things left before review`}
                </Text>
                <ul>
                  {issues.slice(0, 8).map((issue) => (
                    <li key={`${issue.path}/${issue.message}`}>
                      <Anchor
                        href={`#${sectionOf(issue).id}`}
                        size="xs"
                        onClick={(event) => {
                          event.preventDefault()
                          setShowErrors(true)
                          document
                            .getElementById(sectionOf(issue).id)
                            ?.scrollIntoView({ block: 'start' })
                        }}
                      >
                        {sectionOf(issue).title}
                      </Anchor>
                      <Text component="span" size="xs">
                        {' '}
                        {issue.message}
                      </Text>
                    </li>
                  ))}
                </ul>
              </div>
            ) : (
              <Group gap={6} wrap="nowrap" className={classes.ready}>
                <IconCircleCheck size={16} aria-hidden="true" />
                <Text size="sm">Ready to review</Text>
              </Group>
            )}
            <Group gap="sm" wrap="nowrap" className={classes.railActions}>
              <Button variant="default" component={Link} to={trail[trail.length - 1]?.to ?? '/'}>
                Cancel
              </Button>
              <Button
                onClick={review}
                leftSection={<IconEye size={16} aria-hidden="true" />}
                className={classes.reviewButton}
              >
                Review
              </Button>
            </Group>
          </aside>
        </div>
      ) : (
        <Stack gap="lg" className={classes.review}>
          <Title order={2} size="h2">
            Review
          </Title>
          {overlaps.length > 0 && (
            <Alert
              color={overlapBlocksStart ? 'orange' : 'yellow'}
              variant="light"
              icon={<IconAlertTriangle size={18} aria-hidden="true" />}
              title={
                overlapBlocksStart
                  ? 'This campaign overlaps a running one and cannot start yet'
                  : 'This campaign overlaps a running one'
              }
            >
              <Stack gap={4}>
                {overlaps.map((overlap) => (
                  <Text size="sm" key={overlap.campaign.id}>
                    {plural(overlap.nodes, 'node')} {overlap.nodes === 1 ? 'is' : 'are'} also
                    targeted by{' '}
                    <Anchor
                      component={Link}
                      to={`/campaigns/${overlap.campaign.id}`}
                      size="sm"
                      c="inherit"
                      fw={600}
                      underline="always"
                    >
                      {overlap.campaign.name}
                    </Anchor>
                    .
                  </Text>
                ))}
                <Text size="sm">
                  {overlapBlocksStart
                    ? 'You can save it as a draft. To start it, narrow the selector or turn on Allow overlap; only the earlier campaign then acts on those nodes.'
                    : 'Only the earlier campaign acts on those nodes; here their rows become conflicts.'}
                </Text>
              </Stack>
            </Alert>
          )}
          <CampaignDefinitionView definition={definition} matched={matched} />
          {saveFailure !== null && (
            <Alert color="red" variant="light" role="alert" title="twilight did not save it">
              {saveFailure}
            </Alert>
          )}
          <div className={classes.reviewBar}>
            <Button
              variant="default"
              leftSection={<IconArrowLeft size={16} aria-hidden="true" />}
              onClick={() => {
                setView('edit')
                window.scrollTo({ top: 0 })
              }}
              disabled={saving !== null}
            >
              Back to editing
            </Button>
            <Group gap="sm" wrap="wrap" justify="flex-end" className={classes.reviewActions}>
              <Text size="xs" c="dimmed" className={classes.startNote}>
                {startSentence(definition, matched)}
              </Text>
              <Button
                variant="default"
                leftSection={<IconDeviceFloppy size={16} aria-hidden="true" />}
                onClick={() => void save(false)}
                loading={saving === 'draft'}
                disabled={!canOperate || saving !== null}
              >
                {mode.kind === 'edit' ? 'Save changes' : 'Save as draft'}
              </Button>
              <Button
                leftSection={<IconPlayerPlay size={16} aria-hidden="true" />}
                onClick={() => void save(true)}
                loading={saving === 'start'}
                disabled={!canOperate || saving !== null || overlapBlocksStart}
              >
                {mode.kind === 'edit' ? 'Save and start' : 'Create and start'}
              </Button>
            </Group>
          </div>
        </Stack>
      )}
      <Modal
        opened={blocker.state === 'blocked'}
        onClose={() => blocker.reset?.()}
        title="Leave without saving?"
      >
        <Stack gap="md">
          <Text size="sm">
            {mode.kind === 'edit'
              ? 'Your changes to this draft are not saved. Leaving discards them.'
              : 'This campaign is not saved. Leaving discards it.'}
          </Text>
          <Group justify="flex-end" gap="sm">
            <Button variant="default" onClick={() => blocker.reset?.()} data-autofocus>
              Keep editing
            </Button>
            <Button color="red" onClick={() => blocker.proceed?.()}>
              Discard and leave
            </Button>
          </Group>
        </Stack>
      </Modal>
    </>
  )
}

function EditorSkeleton() {
  return (
    <Stack gap="lg" role="status" aria-busy="true" aria-label="Loading the campaign">
      <Skeleton height={28} width="35%" />
      <Skeleton height={140} radius="md" />
      <Skeleton height={220} radius="md" />
      <Skeleton height={320} radius="md" />
    </Stack>
  )
}

function FromCampaign({ campaignId, edit }: { campaignId: string; edit: boolean }) {
  const source = useCampaign(campaignId)
  if (source.isError) {
    if (source.error instanceof ApiError && source.error.status === 404) {
      return <NotFound />
    }
    return (
      <QueryError
        error={source.error}
        what="the campaign to copy"
        onRetry={() => void source.refetch()}
      />
    )
  }
  if (source.data === undefined) {
    return <EditorSkeleton />
  }
  const campaign = source.data
  if (!edit) {
    return (
      <CampaignEditor
        key={campaign.id}
        mode={{ kind: 'create' }}
        initial={{ ...definitionOf(campaign), name: `${campaign.name} (copy)` }}
      />
    )
  }
  if (campaign.status !== 'draft') {
    return (
      <Stack gap="md" maw={640}>
        <PageHeader
          trail={[
            { label: 'Campaigns', to: '/campaigns' },
            { label: campaign.name, to: `/campaigns/${campaign.id}` },
          ]}
          title={`Edit ${campaign.name}`}
          documentTitle={`Edit ${campaign.name}`}
        />
        <Alert color="yellow" variant="light" title="Only a draft can be edited">
          This campaign is {campaign.status}; its definition is fixed. Duplicate it to change the
          definition and run it as a new campaign.
        </Alert>
        <Group>
          <Button component={Link} to={`/campaigns/new?from=${campaign.id}`}>
            Duplicate as a new draft
          </Button>
          <Button component={Link} to={`/campaigns/${campaign.id}`} variant="default">
            Back to the campaign
          </Button>
        </Group>
      </Stack>
    )
  }
  return (
    <CampaignEditor
      key={`${campaign.id}/${campaign.version}`}
      mode={{ kind: 'edit', campaign }}
      initial={definitionOf(campaign)}
    />
  )
}

export function NewCampaignPage() {
  const [searchParameters] = useSearchParams()
  const from = searchParameters.get('from')
  if (from !== null && from.length > 0) {
    return <FromCampaign campaignId={from} edit={false} />
  }
  return <CampaignEditor mode={{ kind: 'create' }} initial={emptyDefinition()} />
}

export function EditCampaignPage() {
  const { campaignId = '' } = useParams()
  return <FromCampaign campaignId={campaignId} edit />
}
