const numberFormat = new Intl.NumberFormat('en-US')

export function formatNumber(value: number): string {
  return numberFormat.format(value)
}

function percentOf(rate: number): number {
  return Math.round(rate * 100 * 1e9) / 1e9
}

export function formatPercent(rate: number | null, distinctFrom?: number): string {
  if (rate === null || !Number.isFinite(rate)) {
    return '–'
  }
  const percent = percentOf(rate)
  if (Number.isInteger(percent)) {
    return `${percent}%`
  }
  const other =
    distinctFrom === undefined || percentOf(distinctFrom) === percent
      ? null
      : formatPercent(distinctFrom)
  let digits = Math.abs(percent) < 1 ? 2 : 1
  let text = percent.toFixed(digits).replace(/\.?0+$/, '')
  while (digits < 9 && (!text.includes('.') || `${text}%` === other)) {
    digits += 1
    text = percent.toFixed(digits).replace(/\.?0+$/, '')
  }
  return `${text}%`
}

export function formatDuration(totalSeconds: number): string {
  if (!Number.isFinite(totalSeconds)) {
    return '–'
  }
  const seconds = Math.max(0, Math.round(totalSeconds))
  if (seconds < 60) {
    return `${seconds}s`
  }
  const days = Math.floor(seconds / 86400)
  const hours = Math.floor((seconds % 86400) / 3600)
  const minutes = Math.floor((seconds % 3600) / 60)
  const remainder = seconds % 60
  if (days > 0) {
    return hours > 0 ? `${days}d ${hours}h` : `${days}d`
  }
  if (hours > 0) {
    return minutes > 0 ? `${hours}h ${minutes}m` : `${hours}h`
  }
  return remainder > 0 && minutes < 10 ? `${minutes}m ${remainder}s` : `${minutes}m`
}

export function formatRelative(iso: string | null, now: number = Date.now()): string {
  if (iso === null) {
    return 'never'
  }
  const time = Date.parse(iso)
  if (Number.isNaN(time)) {
    return iso
  }
  const difference = Math.round((time - now) / 1000)
  const magnitude = Math.abs(difference)
  if (magnitude < 10) {
    return 'just now'
  }
  if (magnitude >= 30 * 86400) {
    return formatDate(iso)
  }
  const text = formatDuration(magnitude).split(' ')[0] ?? ''
  return difference < 0 ? `${text} ago` : `in ${text}`
}

export function formatDate(iso: string): string {
  const date = new Date(iso)
  const sameYear = date.getFullYear() === new Date().getFullYear()
  return date.toLocaleDateString('en-US', {
    month: 'short',
    day: 'numeric',
    ...(sameYear ? {} : { year: 'numeric' }),
  })
}

export function formatAbsolute(iso: string | null): string {
  if (iso === null) {
    return 'never'
  }
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) {
    return iso
  }
  return date.toLocaleString('en-US', {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
    timeZoneName: 'short',
  })
}

export function middleEllipsis(text: string, maximum: number): string {
  if (text.length <= maximum || maximum < 5) {
    return text
  }
  const keep = maximum - 1
  const head = Math.ceil(keep / 2)
  const tail = Math.floor(keep / 2)
  return `${text.slice(0, head)}…${text.slice(text.length - tail)}`
}

export function joinWords(words: string[], conjunction: 'and' | 'or'): string {
  if (words.length <= 1) {
    return words[0] ?? ''
  }
  if (words.length === 2) {
    return `${words[0]} ${conjunction} ${words[1]}`
  }
  return `${words.slice(0, -1).join(', ')} ${conjunction} ${words[words.length - 1]}`
}

export function plural(count: number, singular: string, pluralForm = `${singular}s`): string {
  return `${formatNumber(count)} ${count === 1 ? singular : pluralForm}`
}
