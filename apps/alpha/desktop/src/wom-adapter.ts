/**
 * Stable boundary for a future WOM engine. Domain tables and WOM internals stay
 * out of the desktop foundation until the upstream contract is proven in use.
 */
export interface WomArchiveAdapter {
  readonly contractVersion: string
  importSource(sourcePath: string): Promise<{ objectId: string }>
  readObject(objectId: string): Promise<unknown>
  health(): Promise<'ready' | 'unavailable'>
}

export class DeferredWomAdapter implements WomArchiveAdapter {
  readonly contractVersion = 'deferred'

  async importSource(_sourcePath: string): Promise<{ objectId: string }> {
    throw new Error('WOM adapter contract is waiting for upstream stabilization.')
  }

  async readObject(_objectId: string): Promise<unknown> {
    throw new Error('WOM adapter contract is waiting for upstream stabilization.')
  }

  async health(): Promise<'unavailable'> {
    return 'unavailable'
  }
}
