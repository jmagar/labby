import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'

export default function FeaturesStub(): React.ReactElement {
  return (
    <>
      <h1 className="sr-only">Feature settings</h1>
      <Card>
        <CardHeader>
          <CardTitle>Features</CardTitle>
          <CardDescription>
            v2 stub. Marketplace / MCP Registry / ACP Registry / Chat / Editor /
            Deployments / Activity feature gates ship in v2.
          </CardDescription>
        </CardHeader>
        <CardContent className="text-sm text-muted-foreground">
          Coming in v2 — for now, set per-feature env vars or config.toml entries directly.
        </CardContent>
      </Card>
    </>
  )
}
