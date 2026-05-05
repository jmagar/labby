'use client'

// Left nav rail for /settings/*. Static list of panels; URL-driven
// "active" state via usePathname.

import Link from 'next/link'
import { usePathname } from 'next/navigation'
import { useEffect, useState } from 'react'
import {
  Activity,
  Box,
  Cog,
  FileSearch,
  Layers,
  PlugZap,
  Server,
  Shield,
} from 'lucide-react'

import { cn } from '@/lib/utils'

interface RailEntry {
  href: string
  label: string
  icon: React.ComponentType<{ className?: string }>
  stub?: boolean
}

const ENTRIES: RailEntry[] = [
  { href: '/settings/core/', label: 'Core', icon: Cog },
  { href: '/settings/services/', label: 'Services', icon: Server },
  { href: '/settings/surfaces/', label: 'Surfaces', icon: PlugZap, stub: true },
  { href: '/settings/features/', label: 'Features', icon: Layers, stub: true },
  { href: '/settings/doctor/', label: 'Doctor', icon: Activity },
  { href: '/settings/extract/', label: 'Extract', icon: FileSearch },
  { href: '/settings/advanced/', label: 'Advanced', icon: Shield, stub: true },
]

export function SettingsRail(): React.ReactElement {
  const pathname = usePathname() ?? ''
  const [pluginMode, setPluginMode] = useState(false)
  useEffect(() => {
    try {
      setPluginMode(window.localStorage.getItem('lab.wizard.mode') === 'plugin')
    } catch {
      setPluginMode(false)
    }
  }, [])
  const entries = pluginMode ? ENTRIES.filter((entry) => entry.href === '/settings/services/') : ENTRIES
  return (
    <nav aria-label="Settings sections" className="flex gap-1 overflow-x-auto p-3 lg:flex-col lg:overflow-visible">
      <h2 className="mb-2 hidden items-center gap-2 text-sm font-semibold uppercase text-muted-foreground lg:flex">
        <Box className="h-4 w-4" /> Settings
      </h2>
      {entries.map((entry) => {
        const active = pathname.startsWith(entry.href)
        const Icon = entry.icon
        return (
          <Link
            key={entry.href}
            href={entry.href}
            aria-current={active ? 'page' : undefined}
            className={cn(
              'flex shrink-0 items-center gap-2 rounded-md px-3 py-2 text-sm transition-colors lg:shrink',
              active
                ? 'bg-accent text-accent-foreground font-medium'
                : 'text-muted-foreground hover:bg-accent/50 hover:text-foreground',
            )}
          >
            <Icon className="h-4 w-4" />
              <span className="whitespace-nowrap">{entry.label}</span>
            {entry.stub ? (
              <span className="ml-auto rounded bg-muted px-1.5 py-0.5 text-[10px] uppercase">
                v2
              </span>
            ) : null}
          </Link>
        )
      })}
      {pluginMode ? (
        <Link
          href="/setup/welcome/?mode=full"
          className="flex shrink-0 items-center gap-2 rounded-md px-3 py-2 text-sm text-muted-foreground hover:bg-accent/50 hover:text-foreground lg:shrink"
        >
          Show advanced setup
        </Link>
      ) : null}
    </nav>
  )
}
