# Aurora components — full inventory

This is the orienting map. Verify exact inventory and exported props from `~/workspace/aurora-design-system/registry.json` and the source files — components evolve.

## UI primitives — `registry/aurora/ui/<name>.tsx`

Import path inside the source repo: `@/registry/aurora/ui/<name>`. In a consuming project, install via the shadcn registry (`@aurora/aurora-<name>`) and the components land at the consumer's chosen alias.

### Controls

| Component | File | Notable props / variants |
|---|---|---|
| `Button` | `button.tsx` | `variant`: `aurora` (default, cyan glow), `neutral`, `rose`, `ghost`, `destructive`, `plain`. `size`: `sm`, `default`, `lg`, `icon`, `unstyled`. Use `asChild` to compose. |
| `ButtonGroup` | `button-group.tsx` | Horizontal cluster of Buttons with shared borders. |
| `Badge` | `badge.tsx` | `tone`: `info`, `success`, `warn`, `error`, `neutral`, `rose`, `violet`. Style B is canonical. |
| `Switch` | `switch.tsx` | Boolean toggle. Cyan accent on thumb. |
| `Toggle` | `toggle.tsx` | Single press-state button. |
| `ToggleGroup` | `toggle-group.tsx` | Group of toggles, `type="single"` or `"multiple"`. |
| `Avatar` | `avatar.tsx` | Size variants, status dot composition. |
| `Progress` | `progress.tsx` | Linear progress with cyan accent. |
| `Spinner` | `spinner.tsx` | Indeterminate. Inter-friendly, no spin emoji. |
| `Separator` | `separator.tsx` | Horizontal/vertical rule with `--aurora-border-default`. |
| `Toolbar` | `toolbar.tsx` | Tier-1 toolbar wrapper. |
| `Kbd` | `kbd.tsx` | Keyboard chip in mono. |
| `Accordion` | `accordion.tsx` | Radix-backed disclosure. |

### Form elements

| Component | File | Notes |
|---|---|---|
| `Field` | `field.tsx` | Label + control + description + error wrapper. Prefer over loose Label+Input. |
| `Label` | `label.tsx` | 12px Inter 650. |
| `Input` | `input.tsx` | Supports `startAdornment` / `endAdornment` for icons. |
| `InputGroup` | `input-group.tsx` | Inputs with leading/trailing buttons or addons. |
| `InputOTP` | `input-otp.tsx` | 6-digit OTP. |
| `NativeSelect` | `native-select.tsx` | Use when the surface needs the OS picker. |
| `Select` | `select.tsx` | Radix-backed combobox-style select. |
| `Combobox` | `combobox.tsx` | Filterable select. |
| `DatePicker` | `date-picker.tsx` | Wraps `Calendar`. |
| `Calendar` | `calendar.tsx` | react-day-picker styled to Aurora. |
| `Textarea` | `textarea.tsx` | Same chrome as `Input`. |
| `Checkbox` | `checkbox.tsx` | Custom cyan-checked indicator. |
| `RadioGroup` | `radio-group.tsx` | Set of radios. |
| `Slider` | `slider.tsx` | Uses `.aurora-slider` track + thumb. |
| `NumberInput` | `number-input.tsx` | Stepper buttons. |
| `Tabs` | `tabs.tsx` | Cyan-underline active state. |

### Feedback

| Component | File | Notes |
|---|---|---|
| `Banner` | `banner.tsx` | Style A1 (elevated + glowing dot + dismiss) or Style C (mono tag + inline). Style B was removed. |
| `Callout` | `callout.tsx` | Inline alert/callout. |
| `Toast` | `toast.tsx` | Sonner-style. Dismiss `x` colored by status. Uses Labby mark where available. |
| `Tooltip` | `tooltip.tsx` | Radix tooltip, popover-strong background. |
| `EmptyState` | `empty-state.tsx` | Icon + title + description + action slot. |
| `Skeleton` | `skeleton.tsx` | Shimmer placeholder. |

### Navigation

| Component | File | Notes |
|---|---|---|
| `Breadcrumb` | `breadcrumb.tsx` | Badge sits **left** of item name. |
| `NavigationMenu` | `navigation-menu.tsx` | Radix navigation menu. |
| `Menubar` | `menubar.tsx` | Application menubar. |
| `Pagination` | `pagination.tsx` | Page selector. |
| `ScrollArea` | `scroll-area.tsx` | Custom scrollbars matching tokens. |
| `Listbox` | `listbox.tsx` | Selectable list. |

### Data

| Component | File | Notes |
|---|---|---|
| `Card` | `card.tsx` | Generic Tier 1 card. |
| `Item` | `item.tsx` | List item with leading/trailing slots. |
| `StatCard` | `stat-card.tsx` | `label`, `value`, `delta`, `deltaPositive`, `description`, `tone`. Use in a narrow track grid, not full width. |
| `Table` | `table.tsx` | Primitive table with mono cell variant. |
| `DataTable` | `data-table.tsx` | Sortable, filterable, paginated table. 8px wrapper radius. |
| `Chart` | `chart.tsx` | Recharts wrapper, Aurora palette. |
| `Carousel` | `carousel.tsx` | Embla carousel. |
| `FilterBar` | `filter-bar.tsx` | Pill bar with rose active state for selected filters. |
| `StatusIndicator` | `status-indicator.tsx` | Dot + label, status family aware. |
| `Timeline` | `timeline.tsx` | Vertical event timeline. |
| `DescriptionList` | `description-list.tsx` | Key-value list. |
| `SearchResults` | `search-results.tsx` | Grouped result list. |

### Overlays

| Component | File | Notes |
|---|---|---|
| `Dialog` | `dialog.tsx` | Centered modal on Tier 2 panel. |
| `AlertDialog` | `alert-dialog.tsx` | Confirm/destroy dialog. |
| `DropdownMenu` | `dropdown-menu.tsx` | Radix dropdown. |
| `ContextMenu` | `context-menu.tsx` | Right-click menu. |
| `HoverCard` | `hover-card.tsx` | Lazy preview on hover. |
| `Popover` | `popover.tsx` | Generic popover. |
| `Sheet` | `sheet.tsx` | Side drawer. |
| `Collapsible` | `collapsible.tsx` | Show/hide region. |

### Layout / utility

| Component | File | Notes |
|---|---|---|
| `AspectRatio` | `aspect-ratio.tsx` | Maintain ratio for media. |
| `Direction` | `direction.tsx` | LTR/RTL provider. |
| `ResizablePanels` | `resizable-panels.tsx` | Split-pane container. |

## Product blocks — `registry/aurora/blocks/<domain>/<name>/`

Blocks compose primitives + domain logic. Import path: `@/registry/aurora/blocks/<domain>/<name>/<name>`.

### AI

| Block | Path | What it is |
|---|---|---|
| `PromptInput` | `ai/prompt-input/prompt-input` | Multi-line input with attachments, slash commands, `@` mentions, model selector, send/stop. Props include `value`, `onChange`, `onSubmit`, `attachments`, `slashCommands`, `mentionItems`, `model`, `isStreaming`. |
| `Thinking` | `ai/thinking/thinking` | Reasoning disclosure panel. |
| `ToolCalls` | `ai/tool-calls/tool-calls` | Tool-invocation list with status. |
| `Artifact` | `ai/artifact/artifact` | Generated artifact preview + actions. |
| `AskUserQuestion` | `ai/ask-user-question/ask-user-question` | Inline question card with options. |
| `elements/*` | `ai/elements/` | Smaller AI elements (message, citation, sources, suggestion, agent, etc.). |

### Auth

| Block | Path |
|---|---|
| `Login` | `auth/login/login` |
| `OAuth` | `auth/oauth/oauth` |
| `PermissionPrompt` | `auth/permission-prompt/permission-prompt` |
| `PermissionsDropdown` | `auth/permissions-dropdown/permissions-dropdown` |

### Feedback

| Block | Path |
|---|---|
| `ErrorPage` | `feedback/error-page/error-page` |

### Files

| Block | Path |
|---|---|
| `Attachment` | `files/attachment/attachment` |
| `FilePicker` | `files/file-picker/file-picker` |
| `FileTree` | `files/file-tree/file-tree` |
| `CodeEditor` | `files/code-editor/code-editor` |

### Navigation

| Block | Path |
|---|---|
| `Terminal` | `navigation/terminal/terminal` (Aurora-native chrome — no macOS dots) |

### Workspace

| Block | Path |
|---|---|
| `Sidebar` | `workspace/sidebar/sidebar` |
| `CommandPalette` | `workspace/command-palette/command-palette` |
| `CodeBlock` | `workspace/code-block/code-block` |
| `WebPreview` | `workspace/web-preview/web-preview` |
| `ShareDialog` | `workspace/share-dialog/share-dialog` |
| `Marketplace` | `workspace/marketplace/` (catalog blocks) |

## When to reach for a block vs. a primitive

- **Primitive** when you need *one* control and want full layout control around it.
- **Block** when you're building a known recurring product surface (an agent prompt, a terminal, an attachment list, a permissions UI). Blocks bake in the spacing, the icon set, the empty/loading states, and the resolved decisions. Hand-rolling a prompt input next to the registry `PromptInput` is how surface drift starts.
