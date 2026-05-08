import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'

export default function AdvancedStub(): React.ReactElement {
  return (
    <>
      <h1 className="sr-only">Advanced settings</h1>
      <Card>
        <CardHeader>
          <CardTitle>Advanced</CardTitle>
          <CardDescription>
            v2 stub. Raw Editor (codemirror over .env + config.toml) ships in v2;
            per-service advanced disclosure is exposed inline on each service form already.
          </CardDescription>
        </CardHeader>
        <CardContent className="text-sm text-muted-foreground">Coming in v2.</CardContent>
      </Card>
    </>
  )
}
