/** Which entity a dashboard drill-down targets. UI state, not server data. */
export type DrillTarget =
  | { type: 'tool'; name: string }
  | { type: 'agent'; id: string }
