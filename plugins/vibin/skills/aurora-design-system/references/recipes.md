# Aurora recipes — copy-pasteable patterns

Read this when you're composing a new surface. Each recipe is the *minimum* correct shape — extend, but don't subtract the token usage.

## Page shell

The outermost wrapper for any page that owns its background.

```tsx
export default function Page() {
  return (
    <main className="aurora-page-shell min-h-screen">
      {/* page content */}
    </main>
  )
}
```

## Two-pane operator layout

Sidebar (Tier 0 nav) + main content. The sidebar uses `.aurora-nav-shell`; main content sits on the page shell.

```tsx
import { Sidebar } from "@/registry/aurora/blocks/workspace/sidebar/sidebar"

export default function Layout({ children }: { children: React.ReactNode }) {
  return (
    <div className="aurora-page-shell flex min-h-screen">
      <aside className="aurora-nav-shell w-[260px] shrink-0">
        <Sidebar />
      </aside>
      <main className="flex-1 overflow-auto">
        {children}
      </main>
    </div>
  )
}
```

## Tier 2 inspector panel

The canonical workspace panel. Strong border, Tier 2 shadow + inset highlight, 22px radius.

```tsx
<section
  className="rounded-[var(--aurora-radius-3)] p-5"
  style={{
    background: "var(--aurora-panel-strong)",
    border: "1px solid var(--aurora-border-strong)",
    boxShadow: "var(--aurora-shadow-strong), inset 0 1px 0 rgba(255,255,255,0.05)",
  }}
>
  <h2 className="aurora-text-section">Active gateways</h2>
  {/* ... */}
</section>
```

## Tier 1 toolbar / header

```tsx
<header
  className="rounded-[var(--aurora-radius-2)] flex items-center gap-3 px-4 py-2.5"
  style={{
    background: "var(--aurora-panel-medium)",
    border: "1px solid var(--aurora-border-default)",
    boxShadow: "var(--aurora-shadow-medium), inset 0 1px 0 rgba(255,255,255,0.035)",
  }}
>
  <span className="aurora-text-eyebrow text-[var(--aurora-text-muted)]">Workspace</span>
  <Separator orientation="vertical" />
  <Breadcrumb {...} />
  <div className="ml-auto flex items-center gap-2">
    <Button variant="ghost" size="sm">Filter</Button>
    <Button variant="aurora" size="sm">Run</Button>
  </div>
</header>
```

## Stat grid — narrow tracks, never full-width

```tsx
import { StatCard } from "@/registry/aurora/ui/stat-card"

<div
  className="grid gap-3"
  style={{ gridTemplateColumns: "repeat(auto-fill, minmax(175px, 220px))" }}
>
  <StatCard label="Active jobs" value="42" delta="+3" deltaPositive tone="info" />
  <StatCard label="Failed" value="2" delta="-1" deltaPositive={false} tone="error" />
  <StatCard label="P95 latency" value="186 ms" tone="neutral" />
</div>
```

## Status row — badge + meta

```tsx
import { Badge } from "@/registry/aurora/ui/badge"

<div className="flex items-center gap-2">
  <Badge tone="success">Online</Badge>
  <span className="aurora-text-code text-[var(--aurora-text-muted)]">gw-prod-01</span>
  <span className="aurora-text-meta">updated 2m ago</span>
</div>
```

Note: the ID is mono (`.aurora-text-code`) because it's an identifier; the timestamp is `.aurora-text-meta`.

## Selected list item — border + glow, not flooded fill

```tsx
<button
  className="rounded-[var(--aurora-radius-2)] w-full px-3 py-2 text-left"
  style={
    isSelected
      ? {
          background: "var(--aurora-selected-bg)",
          border: "1px solid var(--aurora-border-strong)",
          boxShadow: "var(--aurora-active-glow)",
        }
      : {
          background: "transparent",
          border: "1px solid var(--aurora-border-default)",
        }
  }
>
  <span className="aurora-text-ui">{item.name}</span>
</button>
```

## Left-border in-progress indicator

```tsx
<li
  className="pl-3"
  style={{ borderLeft: "3px solid var(--aurora-accent-primary)" }}
>
  <span className="aurora-text-ui">Indexing…</span>
</li>
```

Don't use `box-shadow` for the left edge on a rounded element — the glow rounds with the corner and looks wrong.

## Agent prompt with model selector

```tsx
import { PromptInput } from "@/registry/aurora/blocks/ai/prompt-input/prompt-input"

const [value, setValue] = React.useState("")
const [model, setModel] = React.useState("claude-opus-4-7")

<PromptInput
  value={value}
  onChange={setValue}
  onSubmit={(text, attachments) => runAgent(text, attachments)}
  model={model}
  onModelChange={setModel}
  isStreaming={running}
  slashCommands={[
    { id: "plan", label: "/plan", description: "Generate a plan" },
    { id: "search", label: "/search", description: "Search the web" },
  ]}
  mentionItems={files}
  placeholder="Ask the gateway agent…"
/>
```

The send button uses rose by convention — the block handles that internally.

## Data table with filter bar

```tsx
import { DataTable } from "@/registry/aurora/ui/data-table"
import { FilterBar } from "@/registry/aurora/ui/filter-bar"

<section
  className="rounded-[var(--aurora-radius-3)] overflow-hidden"
  style={{
    background: "var(--aurora-panel-strong)",
    border: "1px solid var(--aurora-border-strong)",
    boxShadow: "var(--aurora-shadow-strong), inset 0 1px 0 rgba(255,255,255,0.05)",
  }}
>
  <FilterBar
    filters={filters}
    activeIds={active}
    onToggle={toggle}
  />
  <div style={{ borderRadius: 8, overflow: "hidden" }}>
    <DataTable columns={columns} data={rows} />
  </div>
</section>
```

The active filter chip is rose — `FilterBar` handles it via the `rose` badge tone.

## Banner — A1 elevated

```tsx
import { Banner } from "@/registry/aurora/ui/banner"

<Banner
  tone="error"
  title="Backend unavailable."
  description="Couldn't reach gateway. Retrying in 8s."
  onDismiss={() => dismiss()}
/>
```

Copy is sentence case, ends with a period, no exclamation, no "we".

## Banner — Style C inline mono tag

For inline notices inside dense table rows or compact areas.

```tsx
<div className="flex items-center gap-2 px-3 py-1.5"
  style={{ background: "var(--aurora-warn-surface)", border: "1px solid var(--aurora-warn-border)", borderRadius: "var(--aurora-radius-1)" }}>
  <span className="aurora-text-code uppercase" style={{ color: "var(--aurora-warn-foreground)" }}>
    DEGRADED
  </span>
  <span className="aurora-text-body-sm">Two upstreams responding above p95.</span>
</div>
```

## Tinted accent surface

When you need a tinted panel — agent reply, AI artifact, callout — use `color-mix` against an accent so it tracks light/dark automatically.

```tsx
<div
  className="rounded-[var(--aurora-radius-2)] p-4"
  style={{
    background: "color-mix(in srgb, var(--aurora-accent-violet) 12%, var(--aurora-panel-medium))",
    border: "1px solid color-mix(in srgb, var(--aurora-accent-violet) 32%, transparent)",
  }}
>
  <span className="aurora-text-eyebrow" style={{ color: "var(--aurora-accent-violet-strong)" }}>
    AI
  </span>
  <p className="aurora-text-body">…</p>
</div>
```

Or use the prebuilt `--aurora-accent-violet-surface` and `--aurora-accent-violet-border` directly. Same pattern for cyan, rose, and every status family.

## Empty state

```tsx
import { EmptyState } from "@/registry/aurora/ui/empty-state"
import { Inbox } from "lucide-react"

<EmptyState
  icon={<Inbox size={28} strokeWidth={1.5} />}
  title="No active gateways."
  description="Start one from the marketplace, or run `lab gateway start <name>`."
  action={<Button variant="aurora">Open marketplace</Button>}
/>
```

Note the period on the title — Aurora copy treats empty-state titles as full statements.

## Verifying in light mode

Before shipping, toggle `.light` and re-verify the same surface:

```ts
document.documentElement.classList.toggle("dark")
document.documentElement.classList.toggle("light")
```

Things to look for: enough contrast on muted text, that the accent stays readable, that shadows still feel like real elevation (light-mode shadows are subtler — that's intentional).
