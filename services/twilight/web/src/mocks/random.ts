export class Random {
  private state: number

  constructor(seed: number) {
    this.state = seed >>> 0
  }

  next(): number {
    this.state = (this.state + 0x6d2b79f5) >>> 0
    let mixed = this.state
    mixed = Math.imul(mixed ^ (mixed >>> 15), mixed | 1)
    mixed ^= mixed + Math.imul(mixed ^ (mixed >>> 7), mixed | 61)
    return ((mixed ^ (mixed >>> 14)) >>> 0) / 4294967296
  }

  integer(minimum: number, maximum: number): number {
    return minimum + Math.floor(this.next() * (maximum - minimum + 1))
  }

  chance(probability: number): boolean {
    return this.next() < probability
  }

  pick<Item>(items: readonly Item[]): Item {
    const item = items[Math.floor(this.next() * items.length)]
    if (item === undefined) {
      throw new Error('cannot pick from an empty list')
    }
    return item
  }

  weighted<Item>(entries: readonly (readonly [Item, number])[]): Item {
    const total = entries.reduce((sum, [, weight]) => sum + weight, 0)
    let remaining = this.next() * total
    for (const [item, weight] of entries) {
      remaining -= weight
      if (remaining < 0) {
        return item
      }
    }
    const last = entries[entries.length - 1]
    if (last === undefined) {
      throw new Error('cannot pick from an empty list')
    }
    return last[0]
  }

  hex(length: number): string {
    let text = ''
    while (text.length < length) {
      text += Math.floor(this.next() * 16).toString(16)
    }
    return text
  }

  pid(): string {
    return BigInt(`0x${this.integer(1, 15).toString(16)}${this.hex(15)}`).toString()
  }

  uuid(version: number): string {
    const raw = this.hex(32)
    const variant = ((parseInt(raw[16] ?? '0', 16) & 0x3) | 0x8).toString(16)
    return `${raw.slice(0, 8)}-${raw.slice(8, 12)}-${version}${raw.slice(13, 16)}-${variant}${raw.slice(17, 20)}-${raw.slice(20, 32)}`
  }
}

export function bucketOf(text: string): number {
  let hash = 0x811c9dc5
  for (let index = 0; index < text.length; index += 1) {
    hash ^= text.charCodeAt(index)
    hash = Math.imul(hash, 0x01000193) >>> 0
  }
  return hash / 4294967296
}
