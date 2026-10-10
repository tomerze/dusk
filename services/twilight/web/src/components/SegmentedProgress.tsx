import { Box, Tooltip } from '@mantine/core'
import { formatNumber, formatPercent } from '../format'
import type { StateCounts } from './progress'
import { describeProgress, progressSegments, segmentLabel } from './progress'
import { nodeStateStyles } from './status'
import classes from './SegmentedProgress.module.css'

interface SegmentedProgressProperties {
  counts: StateCounts
  expected?: number | null
  phases?: number[]
  size?: 'sm' | 'md' | 'lg'
}

export function SegmentedProgress({
  counts,
  expected = null,
  phases = [],
  size = 'md',
}: SegmentedProgressProperties) {
  const segments = progressSegments(counts, expected)
  const description = describeProgress(segments)
  return (
    <Box className={classes.root} data-size={size} role="img" aria-label={description} tabIndex={0}>
      <div className={classes.track}>
        {segments.map((segment) => (
          <Tooltip
            key={segment.state}
            label={`${segmentLabel(segment)}: ${formatNumber(segment.count)} (${formatPercent(segment.fraction)})`}
          >
            <div
              className={classes.segment}
              data-state={segment.state}
              style={{
                flexGrow: segment.fraction,
                background:
                  segment.state === 'not_reached'
                    ? undefined
                    : `var(--mantine-color-${nodeStateStyles[segment.state].color}-filled)`,
              }}
              data-testid={`segment-${segment.state}`}
              data-count={segment.count}
            />
          </Tooltip>
        ))}
        {segments.length === 0 && <div className={classes.empty} />}
      </div>
      {phases.slice(0, -1).map((percent) => (
        <span
          key={percent}
          className={classes.tick}
          style={{ left: `${percent}%` }}
          aria-hidden="true"
        />
      ))}
    </Box>
  )
}
