'use client'

import type { ReactNode } from 'react'
import { Search, SlidersHorizontal, X } from 'lucide-react'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { cn } from '@/lib/utils'
import {
  AURORA_CONTROL_SURFACE,
  AURORA_MEDIUM_PANEL,
  AURORA_MUTED_LABEL,
  pillTone,
} from '@/components/aurora/tokens'
import { gatewayActionTone } from './gateway-theme'
import type {
  GatewaySourceFacet,
  GatewayStatusFacet,
  GatewayTransportFacet,
  ToolFilterState,
  ToolsExposureFilter,
} from './gateway-list-state'

export interface GatewayFiltersProps {
  mode: 'gateways' | 'tools'
  search: string
  gatewayFilters: {
    status: GatewayStatusFacet[]
    source: GatewaySourceFacet[]
    transport: GatewayTransportFacet[]
  }
  toolFilters: ToolFilterState
  gatewayOptions: Array<{ value: string; label: string }>
  mobileSheetOpen: boolean
  onMobileSheetOpenChange: (open: boolean) => void
  onSearchChange: (value: string) => void
  onGatewayFilterToggle: (group: 'status' | 'source' | 'transport', value: string) => void
  onToolFilterToggle: (group: 'gatewayIds' | 'source' | 'transport', value: string) => void
  onExposureChange: (value: ToolsExposureFilter) => void
  onClearFilters: () => void
}

interface FilterCheckboxProps {
  checked: boolean
  label: string
  onChange: () => void
}

const GATEWAY_STATUS_OPTIONS: Array<{ value: GatewayStatusFacet; label: string }> = [
  { value: 'configured', label: 'Configured' },
  { value: 'healthy', label: 'Healthy' },
  { value: 'disconnected', label: 'Disconnected' },
  { value: 'enabled', label: 'Enabled' },
  { value: 'disabled', label: 'Disabled' },
]

const SOURCE_OPTIONS: Array<{ value: GatewaySourceFacet; label: string }> = [
  { value: 'lab', label: 'Lab' },
  { value: 'custom', label: 'Custom' },
]

const TRANSPORT_OPTIONS: Array<{ value: GatewayTransportFacet; label: string }> = [
  { value: 'stdio', label: 'stdio' },
  { value: 'http', label: 'HTTP' },
]

const EXPOSURE_OPTIONS: Array<{ value: ToolsExposureFilter; label: string }> = [
  { value: 'all', label: 'All' },
  { value: 'exposed', label: 'Exposed only' },
  { value: 'hidden', label: 'Hidden only' },
]

function FilterCheckbox({ checked, label, onChange }: FilterCheckboxProps) {
  return (
    <label className="flex items-center gap-2 text-[13px] leading-[1.2] font-medium text-aurora-text-primary">
      <Checkbox checked={checked} onCheckedChange={onChange} aria-label={label} className="border-aurora-border-strong bg-aurora-control-surface" />
      <span>{label}</span>
    </label>
  )
}

function FilterGroup({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="space-y-2.5">
      <p className={AURORA_MUTED_LABEL}>{label}</p>
      <div className="space-y-2">{children}</div>
    </div>
  )
}

export function GatewayFilters({
  mode,
  search,
  gatewayFilters,
  toolFilters,
  gatewayOptions,
  mobileSheetOpen,
  onMobileSheetOpenChange,
  onSearchChange,
  onGatewayFilterToggle,
  onToolFilterToggle,
  onExposureChange,
  onClearFilters,
}: GatewayFiltersProps) {
  const gatewayHasNonSearchFilters =
    gatewayFilters.status.length > 0 ||
    gatewayFilters.source.length > 0 ||
    gatewayFilters.transport.length > 0

  const toolHasNonSearchFilters =
    toolFilters.gatewayIds.length > 0 ||
    toolFilters.exposure !== 'all' ||
    toolFilters.source.length > 0 ||
    toolFilters.transport.length > 0

  const hasFilters = mode === 'tools'
    ? search.length > 0 || toolHasNonSearchFilters
    : search.length > 0 || gatewayHasNonSearchFilters

  const activeMobilePills = mode === 'tools'
    ? [
        ...toolFilters.gatewayIds
          .map((gatewayId) => gatewayOptions.find((option) => option.value === gatewayId)?.label)
          .filter(Boolean) as string[],
        ...(toolFilters.exposure === 'all' ? [] : [EXPOSURE_OPTIONS.find((option) => option.value === toolFilters.exposure)?.label ?? toolFilters.exposure]),
        ...toolFilters.source.map((value) => SOURCE_OPTIONS.find((option) => option.value === value)?.label ?? value),
        ...toolFilters.transport.map((value) => TRANSPORT_OPTIONS.find((option) => option.value === value)?.label ?? value),
      ]
    : [
        ...gatewayFilters.status.map((value) => GATEWAY_STATUS_OPTIONS.find((option) => option.value === value)?.label ?? value),
        ...gatewayFilters.source.map((value) => SOURCE_OPTIONS.find((option) => option.value === value)?.label ?? value),
        ...gatewayFilters.transport.map((value) => TRANSPORT_OPTIONS.find((option) => option.value === value)?.label ?? value),
      ]

  const filterGroups = (
    <div className="space-y-4">
      {mode === 'gateways' ? (
        <>
          <FilterGroup label="Status">
            {GATEWAY_STATUS_OPTIONS.map((option) => (
              <FilterCheckbox
                key={option.value}
                checked={gatewayFilters.status.includes(option.value)}
                label={option.label}
                onChange={() => onGatewayFilterToggle('status', option.value)}
              />
            ))}
          </FilterGroup>

          <FilterGroup label="Source">
            {SOURCE_OPTIONS.map((option) => (
              <FilterCheckbox
                key={option.value}
                checked={gatewayFilters.source.includes(option.value)}
                label={option.label}
                onChange={() => onGatewayFilterToggle('source', option.value)}
              />
            ))}
          </FilterGroup>

          <FilterGroup label="Transport">
            {TRANSPORT_OPTIONS.map((option) => (
              <FilterCheckbox
                key={option.value}
                checked={gatewayFilters.transport.includes(option.value)}
                label={option.label}
                onChange={() => onGatewayFilterToggle('transport', option.value)}
              />
            ))}
          </FilterGroup>
        </>
      ) : (
        <>
          <FilterGroup label="Server">
            {gatewayOptions.map((option) => (
              <FilterCheckbox
                key={option.value}
                checked={toolFilters.gatewayIds.includes(option.value)}
                label={option.label}
                onChange={() => onToolFilterToggle('gatewayIds', option.value)}
              />
            ))}
          </FilterGroup>

          <FilterGroup label="Exposure">
            <div className="flex flex-wrap gap-2">
              {EXPOSURE_OPTIONS.map((option) => (
                <button
                  key={option.value}
                  type="button"
                  onClick={() => onExposureChange(option.value)}
                  className={cn(
                    'rounded-full border px-3 py-1.5 text-[13px] leading-[1.2] font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/34',
                    pillTone(toolFilters.exposure === option.value),
                  )}
                  aria-pressed={toolFilters.exposure === option.value}
                  aria-label={`Show ${option.label.toLowerCase()}`}
                >
                  {option.label}
                </button>
              ))}
            </div>
          </FilterGroup>

          <FilterGroup label="Source">
            {SOURCE_OPTIONS.map((option) => (
              <FilterCheckbox
                key={option.value}
                checked={toolFilters.source.includes(option.value)}
                label={option.label}
                onChange={() => onToolFilterToggle('source', option.value)}
              />
            ))}
          </FilterGroup>

          <FilterGroup label="Transport">
            {TRANSPORT_OPTIONS.map((option) => (
              <FilterCheckbox
                key={option.value}
                checked={toolFilters.transport.includes(option.value)}
                label={option.label}
                onChange={() => onToolFilterToggle('transport', option.value)}
              />
            ))}
          </FilterGroup>
        </>
      )}
    </div>
  )

  return (
    <>
      <div className="space-y-3 lg:hidden">
        <div data-mobile-search={mode} className="relative">
          <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-aurora-text-muted" />
          <Input
            aria-label={mode === 'tools' ? 'Search tools' : 'Search servers'}
            name={mode === 'tools' ? 'gateway-tools-search-mobile' : 'gateways-search-mobile'}
            placeholder={mode === 'tools' ? 'Search tools' : 'Search servers'}
            value={search}
            onChange={(e) => onSearchChange(e.target.value)}
            className={cn(
              AURORA_CONTROL_SURFACE,
              'h-10 border pl-9 pr-[4.75rem] text-aurora-text-primary placeholder:text-aurora-text-muted',
            )}
          />
          <div className="absolute inset-y-0 right-1 flex items-center gap-1">
            {search ? (
              <Button
                type="button"
                variant="outline"
                size="icon"
                onClick={() => onSearchChange('')}
                className={cn(gatewayActionTone(), 'size-7 rounded-full hover:bg-aurora-hover-bg hover:text-aurora-text-primary')}
                aria-label="Clear search"
              >
                <X className="size-3.5" />
              </Button>
            ) : null}
            <Button
              type="button"
                variant="outline"
                size="icon"
                onClick={() => onMobileSheetOpenChange(!mobileSheetOpen)}
                className={cn(gatewayActionTone(), 'relative size-7 rounded-full hover:bg-aurora-hover-bg hover:text-aurora-text-primary')}
                aria-label="Open filters"
              >
              <SlidersHorizontal className="size-3" />
              {activeMobilePills.length > 0 ? (
                <span className="absolute -top-1 -right-1 rounded-full border border-aurora-accent-primary/35 bg-aurora-accent-primary/14 px-1.5 text-[10px] font-semibold leading-4 text-aurora-accent-strong">
                  {activeMobilePills.length}
                </span>
              ) : null}
            </Button>
          </div>
        </div>

        {activeMobilePills.length > 0 ? (
          <div className="flex flex-wrap gap-2">
            {activeMobilePills.map((label) => (
              <span
                key={label}
                className={cn(
                  'inline-flex items-center rounded-full border px-2.5 py-1 text-[10px] font-medium uppercase tracking-[0.12em]',
                  pillTone(true),
                )}
              >
                {label}
              </span>
            ))}
          </div>
        ) : null}

        {mobileSheetOpen ? (
          <div className={cn(AURORA_MEDIUM_PANEL, 'space-y-4 p-4')}>
            <div className="flex items-center justify-between gap-3">
              <p className={AURORA_MUTED_LABEL}>Filters</p>
              {hasFilters ? (
                <Button
                  variant="outline"
                  size="sm"
                  onClick={onClearFilters}
                  className={cn(gatewayActionTone(), 'h-8 px-3 text-aurora-text-primary hover:bg-aurora-hover-bg')}
                >
                  <X className="mr-1 size-4" />
                  Clear filters
                </Button>
              ) : null}
            </div>
            {filterGroups}
          </div>
        ) : null}
      </div>

      <div className={cn(AURORA_MEDIUM_PANEL, 'hidden space-y-4 p-4 lg:block')}>
        <div className="space-y-1.5">
          <p className={AURORA_MUTED_LABEL}>Search</p>
          <div className="relative">
            <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-aurora-text-muted" />
            <Input
              aria-label={mode === 'tools' ? 'Search tools' : 'Search servers'}
              name={mode === 'tools' ? 'gateway-tools-search' : 'gateways-search'}
              placeholder={mode === 'tools' ? 'Search tools, descriptions, or servers' : 'Search servers, commands, or endpoints'}
              value={search}
              onChange={(e) => onSearchChange(e.target.value)}
              className={cn(
                AURORA_CONTROL_SURFACE,
                'h-11 border pl-9 text-aurora-text-primary placeholder:text-aurora-text-muted',
              )}
            />
          </div>
        </div>

        <div className="flex items-center justify-between gap-3">
          <p className={AURORA_MUTED_LABEL}>Filters</p>
          {hasFilters ? (
            <Button
              variant="outline"
              size="sm"
              onClick={onClearFilters}
              className={cn(gatewayActionTone(), 'h-9 px-3 text-aurora-text-primary hover:bg-aurora-hover-bg')}
            >
              <X className="mr-1 size-4" />
              Clear filters
            </Button>
          ) : null}
        </div>

        {filterGroups}
      </div>
    </>
  )
}
