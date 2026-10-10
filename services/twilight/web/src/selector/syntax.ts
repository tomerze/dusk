export interface SelectorField {
  name: string
  description: string
  versioned?: boolean
}

export const selectorFields: readonly SelectorField[] = [
  { name: 'country', description: 'ISO 3166 country code, from GeoIP or the time zone' },
  { name: 'os_name', description: 'Operating system id, for example debian or windows' },
  { name: 'os_version', description: 'Operating system version' },
  { name: 'os_build', description: 'Build number or kernel release' },
  {
    name: 'dusk_version',
    description: 'Running dusk version (compares as semver)',
    versioned: true,
  },
  {
    name: 'reported_version',
    description: 'Last reported version (compares as semver)',
    versioned: true,
  },
  { name: 'reported_config_hash', description: 'Last reported config hash' },
  { name: 'hardware_class', description: 'Architecture, core and memory bucket' },
  { name: 'tenant', description: 'Tenant bound to the enrollment credential' },
  {
    name: 'credential_kind',
    description: 'fleet_token or install_token: what the node enrolled with',
  },
  {
    name: 'credential_ref',
    description: 'Fleet token name or install token subject it enrolled with',
  },
  { name: 'credential_issuer', description: 'Key id that signed its install token' },
  { name: 'locale', description: 'Node locale, for example en_US' },
  { name: 'hostname', description: 'Node hostname' },
  { name: 'impl', description: 'Dusk impl, for example nix or windows' },
  { name: 'target_arch', description: 'CPU architecture the node was built for' },
  { name: 'lifecycle', description: 'enrolled, active, quarantined, retired or revoked' },
  { name: 'device_id', description: 'Device id (32 hex characters)' },
  { name: 'installation_id', description: 'Installation id (32 hex characters)' },
  {
    name: 'cert_fingerprint',
    description: 'SHA-256 of the current certificate; changes on renewal',
  },
]

export const selectorKeywords = ['and', 'or', 'not', 'in', 'has', 'semver', 'facts'] as const

export const selectorLiterals = ['true', 'false'] as const

export const commonFactKeys: readonly string[] = [
  'dusk.version',
  'dusk.impl',
  'dusk.hostname',
  'dusk.target.arch',
  'dusk.target.os',
  'dusk.target.bits',
  'dusk.config.hash',
  'dusk.device.cores',
  'dusk.device.memory_bytes',
  'dusk.device.swap_bytes',
  'dusk.device.vendor',
  'dusk.device.model',
  'dusk.device.cpu',
  'dusk.os.time_zone',
  'dusk.os.locale',
  'dusk.os.linux.os_release.id',
  'dusk.os.linux.os_release.version_id',
  'dusk.os.linux.glibc_version',
  'dusk.os.nix.uname.release',
  'dusk.os.windows.build_number',
  'dusk.os.windows.edition',
  'dusk.os.windows.display_version',
  'dusk.os.macos.product_version',
  'dusk.os.android.release',
  'dusk.os.android.sdk',
  'dusk.os.android.brand',
]

export function codePointOffsetToIndex(text: string, offset: number): number {
  let index = 0
  let points = 0
  while (index < text.length && points < offset) {
    const code = text.codePointAt(index) ?? 0
    index += code > 0xffff ? 2 : 1
    points += 1
  }
  return index
}

export function indexToCodePointOffset(text: string, index: number): number {
  return Array.from(text.slice(0, index)).length
}
