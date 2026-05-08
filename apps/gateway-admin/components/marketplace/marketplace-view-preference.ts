export type MarketplaceViewMode = 'cards' | 'table'
export type MarketplaceViewPreference = 'auto' | MarketplaceViewMode

export const MARKETPLACE_VIEW_MODE_STORAGE_KEY = 'labby:marketplace:view-mode'

export function isMarketplaceViewPreference(value: string | null): value is MarketplaceViewPreference {
  return value === 'auto' || value === 'cards' || value === 'table'
}

export function resolveMarketplaceViewMode(
  preference: MarketplaceViewPreference,
  prefersDesktopLayout: boolean,
): MarketplaceViewMode {
  if (preference !== 'auto') return preference
  return prefersDesktopLayout ? 'cards' : 'table'
}
