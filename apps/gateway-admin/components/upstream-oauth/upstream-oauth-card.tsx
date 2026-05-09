'use client'

import { useEffect, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { upstreamOauthApi } from '@/lib/api/upstream-oauth-client'
import { useUpstreamOauthStatus } from '@/lib/hooks/use-upstream-oauth'

interface UpstreamOauthCardProps {
  name: string
}

export function UpstreamOauthCard({ name }: UpstreamOauthCardProps) {
  const [connecting, setConnecting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const { data: status, mutate } = useUpstreamOauthStatus(name, {
    pollWhilePending: connecting,
  })

  useEffect(() => {
    if (connecting && status?.authenticated) {
      setConnecting(false)
    }
  }, [connecting, status?.authenticated])

  async function handleConnect() {
    setError(null)
    setConnecting(true)
    try {
      const { authorization_url } = await upstreamOauthApi.start(name)
      const popup = window.open(authorization_url, '_blank', 'noopener,noreferrer')
      if (!popup) {
        setConnecting(false)
        setError('Popup blocked — please allow popups for this site and try again')
      }
    } catch (err: unknown) {
      setConnecting(false)
      setError(err instanceof Error ? err.message : 'Failed to start authorization')
    }
  }

  async function handleDisconnect() {
    setError(null)
    try {
      await upstreamOauthApi.clear(name)
      await mutate()
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : 'Failed to disconnect')
    }
  }

  const badge = (() => {
    if (!status) return <Badge variant="outline">Loading…</Badge>
    switch (status.state) {
      case 'connected':
        return <Badge variant="outline" className="border-aurora-success/40 text-aurora-success">Connected</Badge>
      case 'expiring':
        return <Badge variant="outline" className="border-aurora-warn/40 text-aurora-warn">Expiring</Badge>
      case 'expired':
        return <Badge variant="outline" className="border-aurora-error/40 text-aurora-error">Expired</Badge>
      case 'refresh_failed':
        return <Badge variant="outline" className="border-aurora-error/40 text-aurora-error">Refresh failed</Badge>
      case 'discovery_failed':
        return <Badge variant="outline" className="border-aurora-error/40 text-aurora-error">Unavailable</Badge>
      case 'disconnected':
        return <Badge variant="outline" className="text-aurora-text-muted">Disconnected</Badge>
      default:
        if (status.authenticated && status.expires_within_5m)
          return <Badge variant="outline" className="border-aurora-warn/40 text-aurora-warn">Expiring</Badge>
        if (status.authenticated)
          return <Badge variant="outline" className="border-aurora-success/40 text-aurora-success">Connected</Badge>
        return <Badge variant="outline" className="text-aurora-text-muted">Disconnected</Badge>
    }
  })()

  const statusDetail = (() => {
    if (!status) return null
    if (status.refresh_error) return status.refresh_error
    if (status.discovery_error) return status.discovery_error
    if (status.refreshed) return 'Token refreshed'
    if (status.state === 'expired') return 'Access token expired'
    if (status.state === 'connected' && status.discovered_tool_count !== undefined)
      return `${status.exposed_tool_count ?? status.discovered_tool_count} of ${status.discovered_tool_count} tools exposed`
    return null
  })()

  return (
    <Card>
      <CardHeader className="pb-2">
        <div className="flex items-center justify-between gap-2">
          <CardTitle className="text-sm font-medium">{name}</CardTitle>
          {badge}
        </div>
      </CardHeader>
      <CardContent className="flex flex-col gap-2 pt-0">
        {error && <p className="text-xs text-destructive">{error}</p>}
        {statusDetail && <p className="text-xs text-aurora-text-muted">{statusDetail}</p>}
        <div className="flex items-center gap-2">
          {status?.authenticated ? (
            <Button variant="outline" size="sm" onClick={handleDisconnect}>
              Disconnect
            </Button>
          ) : (
            <Button size="sm" onClick={handleConnect} disabled={connecting}>
              {connecting ? 'Waiting…' : 'Connect'}
            </Button>
          )}
          {connecting && (
            <p className="text-xs text-aurora-text-muted">
              Complete authorization in the new tab
            </p>
          )}
        </div>
      </CardContent>
    </Card>
  )
}
