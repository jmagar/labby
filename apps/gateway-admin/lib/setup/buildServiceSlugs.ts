// Static catalog of every service slug that should pre-render under
// /settings/services/[service]/. Required by Next.js `output: 'export'`
// — `generateStaticParams()` must enumerate every slug at build time.
//
// Adding a new service = (a) implement it in lab-apis under feature flag
// + (b) add the slug below + (c) `pnpm build`. Keep in alphabetical
// order so diffs read cleanly.

export const SERVICE_SLUGS = [
  'acp',
  'adguard',
  'apprise',
  'arcane',
  'beads',
  'bytestash',
  'deploy',
  'dozzle',
  'freshrss',
  'fs',
  'glances',
  'gotify',
  'immich',
  'jellyfin',
  'linkding',
  'loggifly',
  'marketplace',
  'memos',
  'navidrome',
  'neo4j',
  'notebooklm',
  'openacp',
  'openai',
  'overseerr',
  'pihole',
  'plex',
  'prowlarr',
  'qbittorrent',
  'qdrant',
  'radarr',
  'sabnzbd',
  'scrutiny',
  'sonarr',
  'stash',
  'tailscale',
  'tautulli',
  'tei',
  'unifi',
  'unraid',
  'uptime_kuma',
] as const

export type ServiceSlug = (typeof SERVICE_SLUGS)[number]

export function isKnownService(slug: string): slug is ServiceSlug {
  return (SERVICE_SLUGS as readonly string[]).includes(slug)
}
