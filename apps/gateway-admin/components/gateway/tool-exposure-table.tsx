'use client'

import { useDeferredValue, useEffect, useMemo, useState } from 'react'
import { AlertTriangle, Asterisk, Eye, EyeOff, Search, SlidersHorizontal, Wrench, X } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import {
  type ExposureViewFilter,
  filterToolsForExposureView,
  getExposureFilterCounts,
  getDraftChangeDescription,
} from '@/lib/api/tool-exposure-draft'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { Badge } from '@/components/ui/badge'
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip'
import type { DiscoveredTool } from '@/lib/types/gateway'

const FILTER_OPTIONS = [
  ['all', 'All'],
  ['enabled', 'Enabled'],
  ['hidden', 'Hidden'],
] as const

interface ToolExposureTableProps {
  tools: DiscoveredTool[]
  exposureLabel: string
  exposeAll: boolean
  manageMode: boolean
  hasDraftChanges: boolean
  isSaving: boolean
  selectedRowToolNames: string[]
  currentExposedToolNames: string[]
  draftSelectedToolNames: string[]
  saveErrorMessage?: string | null
  onExposeAllChange: (checked: boolean) => void
  onManageModeChange: (enabled: boolean) => void
  onRowSelectionChange: (names: string[]) => void
  onBulkEnableSelected: (names: string[]) => void
  onBulkDisableSelected: (names: string[]) => void
  onSaveChanges: () => void
  onCancelChanges: () => void
  searchValue?: string
  onSearchValueChange?: (value: string) => void
  hideSearchAndFilterControls?: boolean
  hideManageModeToggle?: boolean
}

export function ToolExposureTable({
  tools,
  exposureLabel,
  exposeAll,
  manageMode,
  hasDraftChanges,
  isSaving,
  selectedRowToolNames,
  currentExposedToolNames,
  draftSelectedToolNames,
  saveErrorMessage,
  onExposeAllChange,
  onManageModeChange,
  onRowSelectionChange,
  onBulkEnableSelected,
  onBulkDisableSelected,
  onSaveChanges,
  onCancelChanges,
  searchValue,
  onSearchValueChange,
  hideSearchAndFilterControls = false,
  hideManageModeToggle = false,
}: ToolExposureTableProps) {
  const [search, setSearch] = useState('')
  const [filter, setFilter] = useState<ExposureViewFilter>('all')
  const [mobileFilterOpen, setMobileFilterOpen] = useState(false)

  useEffect(() => {
    setMobileFilterOpen(false)
  }, [manageMode])
  const isSearchControlled = searchValue !== undefined
  const effectiveSearch = isSearchControlled ? searchValue : search
  const deferredSearch = useDeferredValue(effectiveSearch)
  const setEffectiveSearch = (next: string) => {
    if (!isSearchControlled) {
      setSearch(next)
    }
    onSearchValueChange?.(next)
  }

  const filteredTools = useMemo(
    () => filterToolsForExposureView(tools, filter, deferredSearch),
    [deferredSearch, filter, tools],
  )

  const filterCounts = useMemo(() => getExposureFilterCounts(tools), [tools])
  const hiddenCount = filterCounts.hidden
  const selectedSet = useMemo(() => new Set(selectedRowToolNames), [selectedRowToolNames])
  const visibleToolNames = filteredTools.map((tool) => tool.name)
  const allVisibleSelected =
    visibleToolNames.length > 0 && visibleToolNames.every((name) => selectedSet.has(name))
  const partiallyVisibleSelected =
    visibleToolNames.some((name) => selectedSet.has(name)) && !allVisibleSelected
  const draftChangeDescription = useMemo(
    () => getDraftChangeDescription(currentExposedToolNames, draftSelectedToolNames),
    [currentExposedToolNames, draftSelectedToolNames],
  )

  const updateRowSelection = (toolName: string, checked: boolean) => {
    if (checked) {
      onRowSelectionChange([...selectedSet, toolName].sort((left, right) => left.localeCompare(right)))
      return
    }

    onRowSelectionChange(
      selectedRowToolNames.filter((name) => name !== toolName),
    )
  }

  const handleSelectAllVisible = (checked: boolean) => {
    if (checked) {
      onRowSelectionChange(
        [...new Set([...selectedRowToolNames, ...visibleToolNames])].sort((left, right) =>
          left.localeCompare(right),
        ),
      )
      return
    }

    onRowSelectionChange(
      selectedRowToolNames.filter((name) => !visibleToolNames.includes(name)),
    )
  }

  const unsavedChangesIndicator = hasDraftChanges ? (
    <TooltipProvider>
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            className="inline-flex size-8 items-center justify-center rounded-full border border-aurora-warn/30 bg-aurora-warn/10 text-aurora-warn transition-colors hover:bg-aurora-warn/15"
            aria-label={draftChangeDescription}
            title={draftChangeDescription}
          >
            <AlertTriangle className="size-4" />
          </button>
        </TooltipTrigger>
        <TooltipContent>{draftChangeDescription}</TooltipContent>
      </Tooltip>
    </TooltipProvider>
  ) : null

  return (
      <div className="space-y-3">
      <div className="flex flex-col gap-3 rounded-aurora-2 border bg-aurora-control-surface/20 p-3">
        {!hideSearchAndFilterControls ? (
        <div className="space-y-3 lg:hidden">
          <div className="relative">
            <Search className="absolute left-3 top-1/2 -translate-y-1/2 size-4 text-aurora-text-muted" />
            <Input
              placeholder="Search tools"
              value={effectiveSearch}
              onChange={(event) => setEffectiveSearch(event.target.value)}
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
                className="relative size-7 rounded-full"
                aria-label="Open filters"
              >
                <SlidersHorizontal className="size-3.5" />
                {filter !== 'all' ? (
                  <span className="absolute -top-1 -right-1 rounded-full border border-aurora-accent-primary/35 bg-aurora-accent-primary/14 px-1.5 text-[10px] font-semibold leading-4 text-aurora-accent-strong">
                    1
                  </span>
                ) : null}
              </Button>
            </div>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            <div className="inline-flex items-center gap-2 rounded-full border bg-aurora-page-bg px-3 py-1 text-sm font-medium">
              <Wrench className="size-4 text-primary" />
              {exposureLabel}
            </div>
            <span className="text-sm text-aurora-text-muted tabular-nums">
              {hiddenCount} hidden
            </span>
            {unsavedChangesIndicator}
          </div>

          {mobileFilterOpen ? (
            <div className="space-y-3 rounded-aurora-2 border bg-aurora-page-bg p-3">
              <div className="flex flex-wrap items-center gap-2">
                {FILTER_OPTIONS.map(([value, label]) => (
                  <Button
                    key={value}
                    variant={filter === value ? 'secondary' : 'outline'}
                    size="sm"
                    className="rounded-full"
                    onClick={() => setFilter(value)}
                  >
                    {label}
                    <Badge variant={filter === value ? 'secondary' : 'outline'} className="ml-1 rounded-full">
                      {filterCounts[value]}
                    </Badge>
                  </Button>
                ))}
              </div>

              {!manageMode ? (
                <Button variant="outline" size="sm" onClick={() => onManageModeChange(true)}>
                  <SlidersHorizontal className="mr-2 size-4" />
                  Manage tools
                </Button>
              ) : (
                <div className="flex flex-wrap items-center gap-2">
                  <div className="flex items-center gap-2 rounded-full border bg-aurora-control-surface/20 px-3 py-1">
                    <span className="text-sm font-medium">Expose all</span>
                    <Switch checked={exposeAll} onCheckedChange={onExposeAllChange} />
                  </div>
                  <Button variant="outline" size="sm" onClick={onCancelChanges}>
                    <X className="mr-2 size-4" />
                    Cancel
                  </Button>
                </div>
              )}
            </div>
          ) : null}
        </div>
        ) : null}

        {!hideSearchAndFilterControls ? (
        <div className="hidden lg:flex flex-col gap-2 lg:flex-row lg:items-center lg:justify-between">
          <div className="relative w-full max-w-md">
            <Search className="absolute left-3 top-1/2 -translate-y-1/2 size-4 text-aurora-text-muted" />
            <Input
              placeholder="Search tools..."
              value={effectiveSearch}
              onChange={(event) => setEffectiveSearch(event.target.value)}
              className="h-9 pl-9"
            />
          </div>

          <div className="flex flex-wrap items-center gap-2">
            <div className="inline-flex items-center gap-2 rounded-full border bg-aurora-page-bg px-3 py-1 text-sm font-medium">
              <Wrench className="size-4 text-primary" />
              {exposureLabel}
            </div>
            <span className="text-sm text-aurora-text-muted tabular-nums">
              {hiddenCount} hidden
            </span>
            {unsavedChangesIndicator}
            {hideManageModeToggle ? null : !manageMode ? (
              <Button variant="outline" size="sm" onClick={() => onManageModeChange(true)}>
                <SlidersHorizontal className="mr-2 size-4" />
                Manage Tools
              </Button>
            ) : (
              <>
                <div className="flex items-center gap-2 rounded-full border bg-aurora-page-bg px-3 py-1">
                  <span className="text-sm font-medium">Expose all</span>
                  <Switch checked={exposeAll} onCheckedChange={onExposeAllChange} />
                </div>
                <Button variant="outline" size="sm" onClick={onCancelChanges}>
                  <X className="mr-2 size-4" />
                  Cancel
                </Button>
              </>
            )}
          </div>
        </div>
        ) : (
        <div className="flex flex-wrap items-center gap-2">
          <div className="inline-flex items-center gap-2 rounded-full border bg-aurora-page-bg px-3 py-1 text-sm font-medium">
            <Wrench className="size-4 text-primary" />
            {exposureLabel}
          </div>
          <span className="text-sm text-aurora-text-muted tabular-nums">
            {hiddenCount} hidden
          </span>
          {unsavedChangesIndicator}
          {!manageMode ? (
            hideManageModeToggle ? null : (
            <Button variant="outline" size="sm" onClick={() => onManageModeChange(true)}>
              <SlidersHorizontal className="mr-2 size-4" />
              Manage Tools
            </Button>
            )
          ) : (
            <>
              <div className="flex items-center gap-2 rounded-full border bg-aurora-page-bg px-3 py-1">
                <span className="text-sm font-medium">Expose all</span>
                <Switch checked={exposeAll} onCheckedChange={onExposeAllChange} />
              </div>
              <Button variant="outline" size="sm" onClick={onCancelChanges}>
                <X className="mr-2 size-4" />
                Cancel
              </Button>
            </>
          )}
        </div>
        )}

        {!hideSearchAndFilterControls ? (
        <div className="hidden lg:flex flex-wrap items-center gap-2">
          {([
            ['all', 'All'],
            ['enabled', 'Enabled'],
            ['hidden', 'Hidden'],
          ] as const).map(([value, label]) => (
            <Button
              key={value}
              variant={filter === value ? 'secondary' : 'outline'}
              size="sm"
              className="rounded-full"
              onClick={() => setFilter(value)}
            >
              {label}
              <Badge variant={filter === value ? 'secondary' : 'outline'} className="ml-1 rounded-full">
                {filterCounts[value]}
              </Badge>
            </Button>
          ))}
        </div>
        ) : null}

        {manageMode && (
          <div className="sticky top-4 z-20 flex flex-col gap-3 rounded-aurora-2 border bg-aurora-page-bg/95 p-3 shadow-sm backdrop-blur lg:flex-row lg:items-center lg:justify-between">
            <div className="space-y-1.5">
              <div className="flex flex-wrap items-center gap-2.5">
                <label htmlFor="select-all-visible" className="inline-flex items-center gap-2 text-sm text-aurora-text-muted">
                  <Checkbox
                    id="select-all-visible"
                    checked={allVisibleSelected ? true : partiallyVisibleSelected ? 'indeterminate' : false}
                    onCheckedChange={(value) => handleSelectAllVisible(value === true)}
                  />
                  Select all visible
                </label>
                <Badge variant="secondary" className="rounded-full">{selectedRowToolNames.length} selected</Badge>
                {unsavedChangesIndicator}
              </div>
              <p className="text-sm text-aurora-text-muted">{draftChangeDescription}</p>
              {saveErrorMessage && (
                <p className="text-sm text-destructive">
                  {saveErrorMessage}
                </p>
              )}
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <Button
                variant="outline"
                size="sm"
                disabled={selectedRowToolNames.length === 0}
                onClick={() => onBulkEnableSelected(selectedRowToolNames)}
              >
                Enable selected
              </Button>
              <Button
                variant="outline"
                size="sm"
                disabled={selectedRowToolNames.length === 0}
                onClick={() => onBulkDisableSelected(selectedRowToolNames)}
              >
                Disable selected
              </Button>
              <Button size="sm" disabled={!hasDraftChanges || isSaving} onClick={onSaveChanges}>
                {isSaving ? 'Saving…' : 'Save changes'}
              </Button>
            </div>
          </div>
        )}
      </div>

      <div className="space-y-3 md:hidden">
        {filteredTools.length === 0 ? (
          <div className="rounded-lg border p-6 text-center text-sm text-aurora-text-muted">
            {tools.length === 0 ? 'No tools discovered' : 'No tools match your search'}
          </div>
        ) : (
          filteredTools.map((tool) => (
            <article
              key={tool.name}
              className={tool.exposed ? 'rounded-lg border p-3' : 'rounded-lg border bg-aurora-control-surface/20 p-3 opacity-80'}
            >
              <div className="flex items-start gap-3">
                <TooltipProvider>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <div className={`mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-full ${
                        tool.exposed
                          ? 'bg-aurora-success/10 text-aurora-success'
                          : 'bg-aurora-control-surface text-aurora-text-muted'
                      }`}>
                        {tool.exposed ? <Eye className="size-4" /> : <EyeOff className="size-4" />}
                      </div>
                    </TooltipTrigger>
                    <TooltipContent>
                      {tool.exposed ? 'Exposed downstream' : 'Filtered by allowlist'}
                    </TooltipContent>
                  </Tooltip>
                </TooltipProvider>

                <div className="min-w-0 flex-1 space-y-2">
                  <div className="flex flex-wrap items-start justify-between gap-2">
                    <code className="break-all text-sm font-mono font-medium">{tool.name}</code>
                    {tool.matched_by ? (
                      <Badge variant="secondary" className="max-w-full gap-1 font-mono text-[11px]">
                        {tool.matched_by.includes('*') && <Asterisk className="size-3" />}
                        <span className="break-all">{tool.matched_by}</span>
                      </Badge>
                    ) : (
                      <span className="text-xs text-aurora-text-muted">No match</span>
                    )}
                  </div>
                  {tool.description && (
                    <p className="text-sm text-aurora-text-muted">{tool.description}</p>
                  )}
                </div>
              </div>
            </article>
          ))
        )}
      </div>

      <div className="aurora-scrollbar hidden max-h-[60vh] overflow-auto rounded-lg border md:block">
        <Table>
          <TableHeader>
            <TableRow className="sticky top-0 z-10 bg-aurora-page-bg">
              {manageMode && <TableHead className="w-[44px]" />}
              <TableHead>Tool</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {filteredTools.length === 0 ? (
              <TableRow>
                <TableCell colSpan={manageMode ? 2 : 1} className="py-8 text-center text-aurora-text-muted">
                  {tools.length === 0 ? 'No tools discovered' : 'No tools match your search'}
                </TableCell>
              </TableRow>
            ) : (
              filteredTools.map((tool) => (
                <TableRow key={tool.name} className={!tool.exposed ? 'opacity-70' : ''}>
                  {manageMode && (
                    <TableCell>
                      <Checkbox
                        checked={selectedSet.has(tool.name)}
                        onCheckedChange={(value) => updateRowSelection(tool.name, value === true)}
                        aria-label={`Select ${tool.name}`}
                      />
                    </TableCell>
                  )}
                  <TableCell>
                    <div className="flex items-start justify-between gap-3">
                      <div className="min-w-0 space-y-0.5">
                        <div className="flex items-center gap-2.5">
                          <span
                            className={`size-2 rounded-full ${tool.exposed ? 'bg-aurora-success' : 'bg-aurora-error'}`}
                            aria-hidden="true"
                          />
                          <code className="text-sm font-mono">{tool.name}</code>
                        </div>
                        <p className="pl-[18px] line-clamp-2 text-[13px] leading-5 text-aurora-text-muted">
                          {tool.description || 'No description provided.'}
                        </p>
                      </div>
                      <Badge
                        variant={tool.exposed ? 'secondary' : 'outline'}
                        className="shrink-0 rounded-full px-2 py-0.5 text-[11px]"
                      >
                        {tool.exposed ? 'On' : 'Off'}
                      </Badge>
                    </div>
                  </TableCell>
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </div>
    </div>
  )
}
