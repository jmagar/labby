'use client'

import { useDeferredValue, useEffect, useMemo, useState } from 'react'
import type { LucideIcon } from 'lucide-react'
import { Eye, EyeOff, Search, SlidersHorizontal, X } from 'lucide-react'

import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'

type PrimitiveFilter = 'all' | 'enabled' | 'hidden'

const FILTER_OPTIONS = [
  ['all', 'All'],
  ['enabled', 'Enabled'],
  ['hidden', 'Hidden'],
] as const satisfies ReadonlyArray<readonly [PrimitiveFilter, string]>

export interface PrimitiveExposureItem {
  name: string
  description?: string
  secondary?: string
  exposed: boolean
}

interface PrimitiveExposureTableProps {
  title: string
  description: string
  searchPlaceholder: string
  manageLabel: string
  emptyLabel: string
  exposureEnabled: boolean
  icon: LucideIcon
  items: PrimitiveExposureItem[]
  searchValue?: string
  onSearchValueChange?: (value: string) => void
  onSaveSelection: (selectedNames: string[] | null) => Promise<void>
}

function sortUnique(values: string[]): string[] {
  return [...new Set(values)].sort((left, right) => left.localeCompare(right))
}

export function PrimitiveExposureTable({
  title,
  description,
  searchPlaceholder,
  manageLabel,
  emptyLabel,
  exposureEnabled,
  icon: Icon,
  items,
  searchValue,
  onSearchValueChange,
  onSaveSelection,
}: PrimitiveExposureTableProps) {
  const [search, setSearch] = useState('')
  const [filter, setFilter] = useState<PrimitiveFilter>('all')
  const [manageMode, setManageMode] = useState(false)
  const [mobileFilterOpen, setMobileFilterOpen] = useState(false)
  const [selectedNames, setSelectedNames] = useState<string[]>([])
  const [selectedRows, setSelectedRows] = useState<string[]>([])
  const [isSaving, setIsSaving] = useState(false)

  const effectiveSearch = searchValue ?? search
  const deferredSearch = useDeferredValue(effectiveSearch)
  const setEffectiveSearch = onSearchValueChange ?? setSearch

  useEffect(() => {
    setSelectedNames(sortUnique(items.filter((item) => item.exposed).map((item) => item.name)))
  }, [items])

  useEffect(() => {
    if (!manageMode) {
      setSelectedRows([])
      setMobileFilterOpen(false)
    }
  }, [manageMode])

  const counts = useMemo(() => {
    const enabled = items.filter((item) => item.exposed).length
    return {
      all: items.length,
      enabled,
      hidden: items.length - enabled,
    }
  }, [items])

  const filteredItems = useMemo(() => {
    const term = deferredSearch.trim().toLowerCase()
    return items.filter((item) => {
      const matchesFilter =
        filter === 'all'
          ? true
          : filter === 'enabled'
            ? item.exposed
            : !item.exposed
      const matchesSearch =
        term.length === 0 ||
        item.name.toLowerCase().includes(term) ||
        item.description?.toLowerCase().includes(term) ||
        item.secondary?.toLowerCase().includes(term)
      return matchesFilter && matchesSearch
    })
  }, [deferredSearch, filter, items])

  const filteredNames = filteredItems.map((item) => item.name)
  const selectedRowSet = useMemo(() => new Set(selectedRows), [selectedRows])
  const draftSet = useMemo(() => new Set(selectedNames), [selectedNames])
  const hasDraftChanges = useMemo(() => {
    const current = sortUnique(items.filter((item) => item.exposed).map((item) => item.name))
    const draft = sortUnique(selectedNames)
    return current.length !== draft.length || current.some((name, index) => name !== draft[index])
  }, [items, selectedNames])
  const allExposed = items.length > 0 && items.every((item) => draftSet.has(item.name))
  const allVisibleSelected =
    filteredNames.length > 0 && filteredNames.every((name) => selectedRowSet.has(name))

  const setAllRowsSelected = (checked: boolean) => {
    if (checked) {
      setSelectedRows(sortUnique([...selectedRows, ...filteredNames]))
      return
    }
    setSelectedRows(selectedRows.filter((name) => !filteredNames.includes(name)))
  }

  const updateRowSelected = (name: string, checked: boolean) => {
    if (checked) {
      setSelectedRows(sortUnique([...selectedRows, name]))
      return
    }
    setSelectedRows(selectedRows.filter((value) => value !== name))
  }

  const applyBulk = (shouldEnable: boolean) => {
    const next = new Set(selectedNames)
    for (const name of selectedRows) {
      if (shouldEnable) {
        next.add(name)
      } else {
        next.delete(name)
      }
    }
    setSelectedNames(sortUnique([...next]))
  }

  const saveDraft = async () => {
    setIsSaving(true)
    try {
      const nextSelection =
        items.length > 0 && items.every((item) => selectedNames.includes(item.name))
          ? null
          : selectedNames
      try {
        await onSaveSelection(nextSelection)
        setManageMode(false)
      } catch {
        return
      }
    } finally {
      setIsSaving(false)
    }
  }

  const resetDraft = () => {
    setSelectedNames(sortUnique(items.filter((item) => item.exposed).map((item) => item.name)))
    setSelectedRows([])
    setManageMode(false)
  }

  return (
    <div className="rounded-lg border bg-aurora-panel-medium p-5">
      <div className="space-y-3">
        <div className="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
          <div>
            <h2 className="text-lg font-semibold">{title}</h2>
            <p className="text-sm text-aurora-text-muted">{description}</p>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <Badge variant="outline" className="rounded-full px-3 py-1 text-aurora-text-muted">
              {items.filter((item) => item.exposed).length}/{items.length}
            </Badge>
            <Badge variant="outline" className="rounded-full px-3 py-1 text-aurora-text-muted">
              {exposureEnabled ? 'Surface enabled' : 'Surface disabled'}
            </Badge>
            {!manageMode ? (
              <Button type="button" variant="outline" size="sm" onClick={() => setManageMode(true)}>
                <SlidersHorizontal className="mr-2 size-4" />
                {manageLabel}
              </Button>
            ) : (
              <>
                <div className="flex items-center gap-2 rounded-full border bg-aurora-page-bg px-3 py-1">
                  <span className="text-sm font-medium">Expose all</span>
                  <Switch
                    checked={allExposed}
                    onCheckedChange={(checked) => {
                      setSelectedNames(checked ? sortUnique(items.map((item) => item.name)) : [])
                    }}
                  />
                </div>
                <Button type="button" variant="outline" size="sm" onClick={resetDraft}>
                  <X className="mr-2 size-4" />
                  Cancel
                </Button>
              </>
            )}
          </div>
        </div>

        <div className="space-y-3 rounded-aurora-2 border bg-aurora-control-surface/20 p-3">
          <div className="space-y-3 lg:hidden">
            <div className="relative">
              <Search className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-muted" />
              <Input
                placeholder={searchPlaceholder}
                value={effectiveSearch}
                onChange={(event) => setEffectiveSearch(event.target.value)}
                name={`${manageLabel.toLowerCase().replace(/\s+/g, '-')}-mobile-search`}
                aria-label={searchPlaceholder}
                className="h-10 pl-9 pr-[4.75rem]"
              />
              <div className="absolute inset-y-0 right-1 flex items-center gap-1">
                {effectiveSearch ? (
                  <Button
                    type="button"
                    variant="outline"
                    size="icon"
                    onClick={() => setEffectiveSearch('')}
                    className="size-7 rounded-full"
                    aria-label="Clear search"
                  >
                    <X className="size-3.5" />
                  </Button>
                ) : null}
                <Button
                  type="button"
                  variant="outline"
                  size="icon"
                  onClick={() => setMobileFilterOpen((current) => !current)}
                  className="size-7 rounded-full"
                  aria-label="Open filters"
                >
                  <SlidersHorizontal className="size-3.5" />
                </Button>
              </div>
            </div>
            {mobileFilterOpen ? (
              <div className="flex flex-wrap items-center gap-2 rounded-aurora-2 border bg-aurora-page-bg p-3">
                {FILTER_OPTIONS.map(([value, label]) => (
                  <Button
                    key={value}
                    type="button"
                    variant={filter === value ? 'secondary' : 'outline'}
                    size="sm"
                    className="rounded-full"
                    onClick={() => setFilter(value)}
                  >
                    {label}
                    <Badge variant={filter === value ? 'secondary' : 'outline'} className="ml-1 rounded-full">
                      {counts[value]}
                    </Badge>
                  </Button>
                ))}
              </div>
            ) : null}
          </div>

          <div className="hidden lg:flex lg:items-center lg:justify-between lg:gap-3">
            <div className="relative w-full max-w-md">
              <Search className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-muted" />
              <Input
                placeholder={searchPlaceholder}
                value={effectiveSearch}
                onChange={(event) => setEffectiveSearch(event.target.value)}
                name={`${manageLabel.toLowerCase().replace(/\s+/g, '-')}-desktop-search`}
                aria-label={searchPlaceholder}
                className="h-9 pl-9"
              />
            </div>
            <div className="flex flex-wrap items-center gap-2">
              {FILTER_OPTIONS.map(([value, label]) => (
                <Button
                  key={value}
                  type="button"
                  variant={filter === value ? 'secondary' : 'outline'}
                  size="sm"
                  className="rounded-full"
                  onClick={() => setFilter(value)}
                >
                  {label}
                  <Badge variant={filter === value ? 'secondary' : 'outline'} className="ml-1 rounded-full">
                    {counts[value]}
                  </Badge>
                </Button>
              ))}
            </div>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            <div className="inline-flex items-center gap-2 rounded-full border bg-aurora-page-bg px-3 py-1 text-sm font-medium">
              <Icon className="size-4 text-primary" />
              {manageLabel}
            </div>
            <span className="text-sm text-aurora-text-muted tabular-nums">
              {counts.hidden} hidden
            </span>
            {hasDraftChanges ? (
              <Badge variant="outline" className="rounded-full border-aurora-warn/30 bg-aurora-warn/10 text-aurora-warn">
                Draft changes
              </Badge>
            ) : null}
          </div>
        </div>
      </div>

      {items.length === 0 ? (
        <div className="py-8 text-center text-aurora-text-muted">
          <Icon className="mx-auto mb-3 size-8 opacity-50" />
          <p>{emptyLabel}</p>
        </div>
      ) : (
        <div className="mt-4 overflow-hidden rounded-lg border">
          <Table>
            <TableHeader>
              <TableRow>
                {manageMode ? (
                  <TableHead className="w-12">
                    <Checkbox checked={allVisibleSelected} onCheckedChange={(value) => setAllRowsSelected(value === true)} />
                  </TableHead>
                ) : null}
                <TableHead>Name</TableHead>
                <TableHead>Status</TableHead>
                <TableHead className="w-[12rem] text-right">Visibility</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {filteredItems.map((item) => {
                const draftVisible = draftSet.has(item.name)
                return (
                  <TableRow key={item.name}>
                    {manageMode ? (
                      <TableCell className="align-top">
                        <Checkbox
                          checked={selectedRowSet.has(item.name)}
                          onCheckedChange={(value) => updateRowSelected(item.name, value === true)}
                        />
                      </TableCell>
                    ) : null}
                    <TableCell className="align-top">
                      <div className="space-y-1">
                        <code className="break-all text-sm font-medium">{item.name}</code>
                        {item.description ? (
                          <p className="text-sm text-aurora-text-muted">{item.description}</p>
                        ) : null}
                        {item.secondary ? (
                          <code className="inline-flex max-w-full truncate rounded bg-aurora-control-surface px-2 py-1 text-xs text-aurora-text-muted">
                            {item.secondary}
                          </code>
                        ) : null}
                      </div>
                    </TableCell>
                    <TableCell className="align-top">
                      <Badge variant={draftVisible ? 'secondary' : 'outline'} className="rounded-full">
                        {draftVisible ? 'Exposed' : 'Hidden'}
                      </Badge>
                    </TableCell>
                    <TableCell className="align-top text-right">
                      {manageMode ? (
                        <Button
                          type="button"
                          size="sm"
                          variant="outline"
                          onClick={() => {
                            setSelectedNames((current) => {
                              const next = new Set(current)
                              if (next.has(item.name)) {
                                next.delete(item.name)
                              } else {
                                next.add(item.name)
                              }
                              return sortUnique([...next])
                            })
                          }}
                        >
                          {draftVisible ? (
                            <>
                              <EyeOff className="mr-2 size-4" />
                              Hide
                            </>
                          ) : (
                            <>
                              <Eye className="mr-2 size-4" />
                              Expose
                            </>
                          )}
                        </Button>
                      ) : (
                        <span className="text-xs text-aurora-text-muted">
                          {draftVisible ? 'Available to clients' : 'Blocked from clients'}
                        </span>
                      )}
                    </TableCell>
                  </TableRow>
                )
              })}
            </TableBody>
          </Table>
        </div>
      )}

      {manageMode ? (
        <div className="mt-4 flex flex-col gap-3 rounded-lg border bg-aurora-control-surface/10 p-4 lg:flex-row lg:items-center lg:justify-between">
          <div className="flex flex-wrap items-center gap-2">
            <Button type="button" variant="outline" size="sm" onClick={() => applyBulk(true)} disabled={selectedRows.length === 0}>
              <Eye className="mr-2 size-4" />
              Expose selected
            </Button>
            <Button type="button" variant="outline" size="sm" onClick={() => applyBulk(false)} disabled={selectedRows.length === 0}>
              <EyeOff className="mr-2 size-4" />
              Hide selected
            </Button>
          </div>
          <Button type="button" size="sm" onClick={saveDraft} disabled={isSaving || !hasDraftChanges}>
            Save changes
          </Button>
        </div>
      ) : null}
    </div>
  )
}
