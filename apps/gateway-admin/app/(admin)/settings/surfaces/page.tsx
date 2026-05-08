import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'

export default function SurfacesStub(): React.ReactElement {
  return (
    <>
      <h1 className="sr-only">Surface settings</h1>
      <Card>
        <CardHeader>
          <CardTitle>Surfaces</CardTitle>
          <CardDescription>
            v2 stub. Surface toggles (Web / CLI / API / MCP / TUI / OAuth Relay)
            are configured via env vars and config.toml directly until v2.
          </CardDescription>
        </CardHeader>
        <CardContent className="text-sm text-muted-foreground">
          Coming in v2 — for now, edit <code>~/.lab/config.toml</code> directly to toggle surfaces.
        </CardContent>
      </Card>
    </>
  )
}
