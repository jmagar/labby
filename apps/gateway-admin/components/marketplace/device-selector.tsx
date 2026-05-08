'use client'

import { useEffect, useState } from 'react'
import { Monitor, Wifi, WifiOff } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { cn } from '@/lib/utils'
import { fetchFleetDevices, type FleetDevice } from '@/lib/api/device-client'

export interface DeviceSelectorProps {
  selected: string[]
  onChange: (deviceIds: string[]) => void
  className?: string
}

const LOCAL_CHAT_DEVICE: FleetDevice = {
  node_id: 'local',
  connected: true,
  role: 'controller',
}

function withLocalChatDevice(devices: FleetDevice[]): FleetDevice[] {
  if (devices.some((device) => device.node_id === LOCAL_CHAT_DEVICE.node_id)) {
    return devices
  }
  return [LOCAL_CHAT_DEVICE, ...devices]
}

function PlatformBadge({
  label,
  enabled,
}: {
  label: string
  enabled: boolean
}) {
  if (enabled) {
    return (
      <Badge variant="outline" className="text-[10px] uppercase tracking-[0.1em] px-1.5 py-px">
        {label}
      </Badge>
    )
  }

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span>
          <Badge
            variant="outline"
            className="text-[10px] uppercase tracking-[0.1em] px-1.5 py-px opacity-50 cursor-not-allowed"
          >
            {label}
          </Badge>
        </span>
      </TooltipTrigger>
      <TooltipContent>Coming soon</TooltipContent>
    </Tooltip>
  )
}

function DeviceRow({
  device,
  selected,
  onToggle,
}: {
  device: FleetDevice
  selected: boolean
  onToggle: (id: string) => void
}) {
  const checkboxId = `device-select-${device.node_id}`
  const isLocalChatTarget = device.node_id === LOCAL_CHAT_DEVICE.node_id

  return (
    <label
      htmlFor={checkboxId}
      className={cn(
        'flex items-center gap-3 px-3 py-2.5 rounded-aurora-1 border cursor-pointer transition-colors',
        selected
          ? 'bg-[color-mix(in_srgb,var(--aurora-accent-primary)_8%,transparent)] border-[color-mix(in_srgb,var(--aurora-accent-primary)_30%,transparent)]'
          : 'bg-aurora-control-surface border-aurora-border-strong hover:bg-aurora-hover-bg',
      )}
    >
      <input
        id={checkboxId}
        type="checkbox"
        className="sr-only"
        checked={selected}
        onChange={() => onToggle(device.node_id)}
      />
      {/* Visual checkbox */}
      <div
        className={cn(
          'flex-shrink-0 w-4 h-4 rounded border flex items-center justify-center transition-colors',
          selected
            ? 'bg-aurora-accent-primary border-aurora-accent-primary'
            : 'bg-transparent border-aurora-border-strong',
        )}
      >
        {selected && (
          <svg className="w-2.5 h-2.5 text-white" fill="none" viewBox="0 0 10 8">
            <path d="M1 4l3 3 5-6" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        )}
      </div>

      {/* Device icon */}
      <Monitor className="w-4 h-4 flex-shrink-0 text-aurora-text-muted" />

      {/* Device info */}
      <div className="flex-1 min-w-0">
        <div className="text-[13px] font-medium text-aurora-text-primary truncate">
          {device.node_id}
        </div>
        <div className="flex items-center gap-1.5 mt-px">
          {device.connected ? (
            <>
              <Wifi className="w-3 h-3 text-aurora-success" />
              <span className="text-[11px] text-aurora-success">Connected</span>
            </>
          ) : (
            <>
              <WifiOff className="w-3 h-3 text-aurora-text-muted" />
              <span className="text-[11px] text-aurora-text-muted">Offline</span>
            </>
          )}
          <span className="text-[11px] text-aurora-text-muted">·</span>
          <span className="text-[11px] text-aurora-text-muted capitalize">{device.role}</span>
        </div>
      </div>

      {/* Platform badges */}
      <div className="flex items-center gap-1 flex-shrink-0">
        {isLocalChatTarget ? (
          <PlatformBadge label="Chat" enabled={true} />
        ) : (
          <>
            <PlatformBadge label="Claude" enabled={true} />
            <PlatformBadge label="Codex" enabled={false} />
            <PlatformBadge label="Gemini" enabled={false} />
          </>
        )}
      </div>
    </label>
  )
}

export function DeviceSelector({ selected, onChange, className }: DeviceSelectorProps) {
  const [devices, setDevices] = useState<FleetDevice[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    const controller = new AbortController()
    setLoading(true)
    setError(null)

    fetchFleetDevices(controller.signal)
      .then(data => {
        const nextDevices = withLocalChatDevice(data)
        setDevices(nextDevices)
        // Pre-select the first device (local/master) if nothing selected yet
        if (selected.length === 0 && nextDevices.length > 0) {
          const localDevice =
            nextDevices.find(d => d.node_id === LOCAL_CHAT_DEVICE.node_id) ??
            nextDevices.find(d => d.role === 'master') ??
            nextDevices[0]
          if (localDevice) {
            onChange([localDevice.node_id])
          }
        }
      })
      .catch(err => {
        if (err instanceof DOMException && err.name === 'AbortError') return
        setError(err instanceof Error ? err.message : 'Failed to load devices')
      })
      .finally(() => setLoading(false))

    return () => controller.abort()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  function handleToggle(id: string) {
    if (selected.includes(id)) {
      onChange(selected.filter(s => s !== id))
    } else {
      onChange([...selected, id])
    }
  }

  if (loading) {
    return (
      <div className={cn('space-y-2', className)}>
        {[0, 1].map(i => (
          <div
            key={i}
            className="h-[62px] rounded-aurora-1 border border-aurora-border-strong bg-aurora-control-surface animate-pulse"
          />
        ))}
      </div>
    )
  }

  if (error) {
    return (
      <div className={cn('rounded-aurora-1 border border-aurora-error/30 bg-aurora-error/8 px-3 py-2.5', className)}>
        <p className="text-[12px] text-aurora-error">{error}</p>
      </div>
    )
  }

  if (devices.length === 0) {
    return (
      <div className={cn('rounded-aurora-1 border border-aurora-border-strong bg-aurora-control-surface px-3 py-4 text-center', className)}>
        <p className="text-[12px] text-aurora-text-muted">No devices found</p>
      </div>
    )
  }

  return (
    <div className={cn('space-y-1.5', className)}>
      {devices.map(device => (
        <DeviceRow
          key={device.node_id}
          device={device}
          selected={selected.includes(device.node_id)}
          onToggle={handleToggle}
        />
      ))}
    </div>
  )
}
