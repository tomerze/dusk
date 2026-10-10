import type { JsonValue, Lifecycle, NodeDetail, NodeSummary, PresenceSession } from '../api/types'
import { Random } from './random'

interface OsProfile {
  name: string
  version: string
  build: string
  family: 'linux' | 'windows' | 'macos' | 'android' | 'freebsd'
  weight: number
}

const osProfiles: readonly OsProfile[] = [
  { name: 'ubuntu', version: '24.04', build: '6.8.0-45-generic', family: 'linux', weight: 18 },
  { name: 'ubuntu', version: '22.04', build: '5.15.0-122-generic', family: 'linux', weight: 11 },
  { name: 'debian', version: '12', build: '6.1.0-26-amd64', family: 'linux', weight: 14 },
  { name: 'debian', version: '13', build: '6.12.9-amd64', family: 'linux', weight: 4 },
  { name: 'alpine', version: '3.20', build: '6.6.54-0-lts', family: 'linux', weight: 3 },
  { name: 'windows', version: '11 23H2', build: '22631.4317', family: 'windows', weight: 12 },
  { name: 'windows', version: '11 24H2', build: '26100.2033', family: 'windows', weight: 10 },
  { name: 'windows', version: '10 22H2', build: '19045.5011', family: 'windows', weight: 6 },
  { name: 'macos', version: '15.1', build: '24B83', family: 'macos', weight: 6 },
  { name: 'macos', version: '14.7', build: '23H124', family: 'macos', weight: 3 },
  { name: 'android', version: '14', build: 'UP1A.231005.007', family: 'android', weight: 6 },
  { name: 'android', version: '15', build: 'AP3A.241005.015', family: 'android', weight: 4 },
  { name: 'freebsd', version: '14.1', build: '14.1-RELEASE-p5', family: 'freebsd', weight: 2 },
]

const countries: readonly (readonly [string, number, string, string])[] = [
  ['US', 28, 'America/Chicago', 'en_US'],
  ['DE', 10, 'Europe/Berlin', 'de_DE'],
  ['GB', 8, 'Europe/London', 'en_GB'],
  ['IN', 8, 'Asia/Kolkata', 'en_IN'],
  ['BR', 7, 'America/Sao_Paulo', 'pt_BR'],
  ['JP', 6, 'Asia/Tokyo', 'ja_JP'],
  ['FR', 5, 'Europe/Paris', 'fr_FR'],
  ['CA', 5, 'America/Toronto', 'en_CA'],
  ['AU', 4, 'Australia/Sydney', 'en_AU'],
  ['NL', 3, 'Europe/Amsterdam', 'nl_NL'],
  ['IL', 3, 'Asia/Jerusalem', 'he_IL'],
  ['MX', 3, 'America/Mexico_City', 'es_MX'],
  ['KR', 3, 'Asia/Seoul', 'ko_KR'],
  ['SE', 2, 'Europe/Stockholm', 'sv_SE'],
  ['ES', 2, 'Europe/Madrid', 'es_ES'],
  ['IT', 2, 'Europe/Rome', 'it_IT'],
  ['ZA', 1, 'Africa/Johannesburg', 'en_ZA'],
]

const tenants: readonly (readonly [string | null, number])[] = [
  ['acme-retail', 45],
  ['northwind-logistics', 30],
  ['contoso-health', 15],
  [null, 10],
]

const duskVersions: readonly (readonly [string, number])[] = [
  ['0.1.0', 9],
  ['0.1.1', 21],
  ['0.2.0', 42],
  ['0.2.1', 22],
  ['0.2.2-rc.1', 6],
]

const lifecycleWeights: readonly (readonly [Lifecycle, number])[] = [
  ['active', 92],
  ['enrolled', 3],
  ['quarantined', 2],
  ['retired', 1.8],
  ['revoked', 1.2],
]

const vendors: Record<OsProfile['family'], readonly (readonly [string, string])[]> = {
  linux: [
    ['Dell Inc.', 'OptiPlex 7010'],
    ['Lenovo', 'ThinkCentre M70q'],
    ['Raspberry Pi Ltd', 'Raspberry Pi 5 Model B'],
    ['Advantech', 'UNO-2271G'],
    ['QEMU', 'Standard PC (Q35 + ICH9, 2009)'],
  ],
  windows: [
    ['HP', 'EliteDesk 800 G6'],
    ['Lenovo', 'ThinkPad T14 Gen 4'],
    ['Dell Inc.', 'Latitude 5440'],
    ['Toshiba', 'TCx 810 POS'],
  ],
  macos: [
    ['Apple Inc.', 'Mac mini (M2, 2023)'],
    ['Apple Inc.', 'MacBook Air (M3, 2024)'],
  ],
  android: [
    ['Zebra', 'TC58'],
    ['Samsung', 'Galaxy Tab Active5'],
    ['Honeywell', 'CT47'],
  ],
  freebsd: [['Supermicro', 'SYS-E300-9D']],
}

export interface MockNode extends NodeDetail {
  remote_address: string
}

function hostnameFor(random: Random, tenant: string | null, family: OsProfile['family']): string {
  const number = (width: number, maximum: number) =>
    String(random.integer(1, maximum)).padStart(width, '0')
  let name: string
  switch (tenant) {
    case 'acme-retail':
      name = `${random.pick(['pos', 'pos', 'kiosk', 'backoffice'])}-${number(4, 2400)}-${number(2, 12)}`
      break
    case 'northwind-logistics':
      name = `${random.pick(['truck', 'dock', 'scanner', 'depot'])}-${number(5, 30000)}`
      break
    case 'contoso-health':
      name = `${random.pick(['ward', 'imaging', 'pharmacy'])}-${number(3, 400)}-${number(2, 40)}`
      break
    default:
      name = `edge-${random.hex(6)}`
  }
  return family === 'windows' ? name.toUpperCase() : name
}

function presenceSessions(
  random: Random,
  now: number,
  count: number,
  lastSeen: number,
): PresenceSession[] {
  return Array.from({ length: count }, () => {
    const instance = random.integer(0, 5)
    const connectedAt = now - random.integer(60, 21 * 86400) * 1000
    return {
      namespace_id: random.hex(16),
      epoch: connectedAt * 1000 + random.integer(0, 999),
      instance: `nightfall-${instance}`,
      inner_address: `nightfall-${instance}.nightfall-inner.dusk.svc:8444`,
      connected_at: new Date(connectedAt).toISOString(),
      last_seen: new Date(lastSeen).toISOString(),
    }
  })
}

interface FactSource {
  dusk_version: string
  impl: string
  hostname: string
  target_arch: string
  locale: string
  tenant: string | null
}

function factsFor(
  random: Random,
  node: FactSource,
  profile: OsProfile,
  cores: number,
  memoryGigabytes: number,
  timeZone: string,
  configHash: string,
): Record<string, JsonValue> {
  const [vendor, model] = random.pick(vendors[profile.family])
  const facts: Record<string, JsonValue> = {
    'dusk.version': node.dusk_version,
    'dusk.impl': node.impl,
    'dusk.hostname': node.hostname,
    'dusk.target.arch': node.target_arch,
    'dusk.target.os': profile.family === 'macos' ? 'macos' : profile.family,
    'dusk.target.bits': 64,
    'dusk.config.hash': configHash,
    'dusk.device.cores': cores,
    'dusk.device.memory_bytes': memoryGigabytes * 1024 * 1024 * 1024,
    'dusk.device.swap_bytes': random.pick([0, 2, 4, 8]) * 1024 * 1024 * 1024,
    'dusk.device.vendor': vendor,
    'dusk.device.model': model,
    'dusk.device.boot_time_ms': Date.now() - random.integer(3600, 40 * 86400) * 1000,
    'dusk.os.time_zone': timeZone,
    'dusk.os.locale': node.locale,
  }
  switch (profile.family) {
    case 'linux':
      facts['dusk.os.linux.os_release.id'] = profile.name
      facts['dusk.os.linux.os_release.version_id'] = profile.version
      facts['dusk.os.linux.os_release.pretty_name'] =
        profile.name === 'ubuntu'
          ? `Ubuntu ${profile.version} LTS`
          : profile.name === 'debian'
            ? `Debian GNU/Linux ${profile.version}`
            : `Alpine Linux v${profile.version}`
      facts['dusk.os.nix.uname.release'] = profile.build
      facts['dusk.os.nix.uname.machine'] = node.target_arch
      if (profile.name !== 'alpine') {
        facts['dusk.os.linux.glibc_version'] = profile.version === '22.04' ? '2.35' : '2.36'
      }
      break
    case 'windows':
      facts['dusk.os.windows.build_number'] = Number(profile.build.split('.')[0])
      facts['dusk.os.windows.revision'] = Number(profile.build.split('.')[1])
      facts['dusk.os.windows.edition'] = random.pick(['Enterprise', 'Pro', 'IoT Enterprise LTSC'])
      facts['dusk.os.windows.display_version'] = profile.version.split(' ')[1] ?? profile.version
      facts['dusk.os.windows.elevated'] = true
      break
    case 'macos':
      facts['dusk.os.macos.product_version'] = profile.version
      facts['dusk.os.macos.build_version'] = profile.build
      facts['dusk.os.macos.translated'] = false
      break
    case 'android':
      facts['dusk.os.android.release'] = profile.version
      facts['dusk.os.android.sdk'] = profile.version === '14' ? 34 : 35
      facts['dusk.os.android.brand'] = vendor
      facts['dusk.os.android.model'] = model
      facts['dusk.os.android.incremental'] = profile.build
      facts['dusk.os.android.security_patch'] = '2024-10-05'
      break
    case 'freebsd':
      facts['dusk.os.nix.uname.release'] = profile.build
      facts['dusk.os.nix.uname.sysname'] = 'FreeBSD'
      break
  }
  if (node.tenant === 'acme-retail') {
    facts['acme.pos.version'] = random.weighted([
      ['4.12.0', 35],
      ['4.13.1', 55],
      ['4.14.0-beta.2', 10],
    ])
    facts['acme.store_id'] = node.hostname?.split('-')[1] ?? '0000'
    facts['acme.ring'] = random.weighted([
      ['beta', 3],
      ['stable', 97],
    ])
  }
  if (node.tenant === 'northwind-logistics') {
    facts['northwind.depot'] = random.pick(['ORD', 'ATL', 'DFW', 'FRA', 'LHR', 'GRU', 'NRT'])
  }
  return facts
}

export function generateNodes(seed: number, count: number, now: number): MockNode[] {
  const random = new Random(seed)
  const nodes: MockNode[] = []
  const usedHostnames = new Set<string>()
  for (let index = 0; index < count; index += 1) {
    const profile = random.weighted(osProfiles.map((entry) => [entry, entry.weight] as const))
    const [country, , timeZone, locale] = random.weighted(
      countries.map((entry) => [entry, entry[1]] as const),
    )
    const tenant = random.weighted(tenants)
    const lifecycle = random.weighted(lifecycleWeights)
    const architecture =
      profile.family === 'android'
        ? 'aarch64'
        : profile.family === 'macos'
          ? random.chance(0.85)
            ? 'aarch64'
            : 'x86_64'
          : profile.family === 'windows'
            ? random.chance(0.9)
              ? 'x86_64'
              : 'aarch64'
            : random.chance(0.75)
              ? 'x86_64'
              : 'aarch64'
    const cores = random.weighted([
      [2, 10],
      [4, 40],
      [8, 30],
      [16, 15],
      [32, 5],
    ] as const)
    const memoryGigabytes = random.weighted([
      [2, 8],
      [4, 25],
      [8, 35],
      [16, 22],
      [32, 8],
      [64, 2],
    ] as const)
    const duskVersion = random.weighted(duskVersions)
    let hostname = hostnameFor(random, tenant, profile.family)
    while (usedHostnames.has(hostname)) {
      hostname = `${hostname}-${random.hex(2)}`
    }
    usedHostnames.add(hostname)
    const online =
      lifecycle === 'retired' || lifecycle === 'revoked'
        ? false
        : random.chance(lifecycle === 'enrolled' ? 0.5 : 0.74)
    const lastSeen = online
      ? now - random.integer(1, 30) * 1000
      : now -
        random.weighted([
          [random.integer(120, 3600), 40],
          [random.integer(3600, 86400), 35],
          [random.integer(86400, 30 * 86400), 25],
        ] as const) *
          1000
    const firstSeen = now - random.integer(20, 420) * 86400 * 1000
    const sessions = online
      ? presenceSessions(random, now, random.chance(0.03) ? 2 : 1, lastSeen)
      : []
    const impl =
      profile.family === 'windows' ? 'windows' : profile.family === 'android' ? 'std' : 'nix'
    const configHash = random.chance(0.8)
      ? 'b41d9a0c3e5f7a2d6c8e1f0a9b7c5d3e2f1a0b9c8d7e6f5a4b3c2d1e0f9a8b7c'
      : '7e0c2b9a4d6f8e1c3a5b7d9f0e2c4a6b8d0f1e3c5a7b9d2f4e6a8c0b1d3f5e7a'
    const deviceId = random.hex(32)
    const installationId = random.hex(32)
    const facts = factsFor(
      random,
      { dusk_version: duskVersion, impl, hostname, target_arch: architecture, locale, tenant },
      profile,
      cores,
      memoryGigabytes,
      timeZone,
      configHash,
    )
    const changedAt =
      lifecycle === 'active' || lifecycle === 'enrolled'
        ? null
        : new Date(now - random.integer(2, 60) * 86400 * 1000).toISOString()
    nodes.push({
      device_id: deviceId,
      installation_id: installationId,
      cert_fingerprint: random.hex(64),
      lifecycle,
      lifecycle_reason:
        lifecycle === 'quarantined'
          ? random.pick([
              'suspected tampering, ticket SEC-2291',
              'cloned disk image',
              'failed attestation review',
            ])
          : lifecycle === 'retired'
            ? random.pick(['hardware returned to vendor', 'store closed', 'replaced by newer unit'])
            : lifecycle === 'revoked'
              ? random.pick([
                  'device reported stolen',
                  'private key exposure',
                  'decommissioned under incident INC-4471',
                ])
              : null,
      lifecycle_changed_at: changedAt,
      country,
      os_name: profile.name,
      os_version: profile.version,
      os_build: profile.build,
      dusk_version: duskVersion,
      hardware_class: `${architecture}-${cores}c-${memoryGigabytes}g`,
      tenant,
      locale,
      hostname,
      impl,
      target_arch: architecture,
      facts,
      reported_version: duskVersion,
      reported_config_hash: configHash,
      reported_services: ['nightfall', 'sh', 'logs', ...(random.chance(0.3) ? ['kvs'] : [])],
      reported_at: new Date(lastSeen - random.integer(0, 3600) * 1000).toISOString(),
      facts_namespace_id: sessions[0]?.namespace_id ?? random.hex(16),
      facts_read_at: new Date(now - random.integer(60, 20 * 3600) * 1000).toISOString(),
      enrolled_at:
        lifecycle === 'enrolled' && random.chance(0.3) ? null : new Date(firstSeen).toISOString(),
      first_seen_at: new Date(firstSeen).toISOString(),
      updated_at: new Date(now - random.integer(60, 86400) * 1000).toISOString(),
      credential_kind: 'fleet_token',
      credential_ref: `${tenant ?? 'lab'}-2026`,
      credential_issuer: null,
      online,
      last_seen_at: new Date(lastSeen).toISOString(),
      sessions,
      executions: [],
      device: null,
      remote_address: `${random.integer(11, 223)}.${random.integer(0, 255)}.${random.integer(0, 255)}.${random.integer(1, 254)}:${random.integer(32768, 60999)}`,
    })
  }
  const cloned = nodes
    .filter((node) => node.tenant === 'acme-retail' && node.os_name === 'debian')
    .slice(0, 38)
  for (const node of cloned) {
    node.facts['acme.image_id'] = 'img-7f3a'
  }
  return nodes
}

export function toSummary(node: MockNode): NodeSummary {
  const {
    sessions: liveSessions,
    executions: rows,
    remote_address: remoteAddress,
    ...summary
  } = node
  return summary
}

export function toDetail(node: MockNode): NodeDetail {
  const { remote_address: remoteAddress, ...detail } = node
  return detail
}

export function selectableFields(node: MockNode): Record<string, string | null> {
  return {
    country: node.country,
    os_name: node.os_name,
    os_version: node.os_version,
    os_build: node.os_build,
    dusk_version: node.dusk_version,
    reported_version: node.reported_version,
    reported_config_hash: node.reported_config_hash,
    hardware_class: node.hardware_class,
    tenant: node.tenant,
    credential_kind: node.credential_kind,
    credential_ref: node.credential_ref,
    credential_issuer: node.credential_issuer,
    locale: node.locale,
    hostname: node.hostname,
    impl: node.impl,
    target_arch: node.target_arch,
    lifecycle: node.lifecycle,
    device_id: node.device_id,
    installation_id: node.installation_id,
    cert_fingerprint: node.cert_fingerprint,
  }
}
