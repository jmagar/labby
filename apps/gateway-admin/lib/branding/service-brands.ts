/**
 * External-service brand identity colors and logos.
 *
 * These are the only sanctioned raw hex values in the UI tree — they encode
 * third-party brand identity (Plex orange, Sonarr cyan, Radarr yellow, etc.)
 * rather than Aurora UI tone. See docs/design/design-system-contract.md
 * "Service Brand Identity" for the carve-out.
 */

export const SERVICE_KEYS = [
  'apprise',
  'arcane',
  'bytestash',
  'gotify',
  'linkding',
  'memos',
  'openai',
  'overseerr',
  'plex',
  'prowlarr',
  'qbittorrent',
  'qdrant',
  'radarr',
  'sabnzbd',
  'sonarr',
  'tailscale',
  'tautulli',
  'tei',
  'unifi',
  'unraid',
] as const

export type ServiceKey = (typeof SERVICE_KEYS)[number]

export function isServiceKey(value: string): value is ServiceKey {
  return (SERVICE_KEYS as readonly string[]).includes(value)
}

const selfhst = (slug: string) =>
  `https://cdn.jsdelivr.net/gh/selfhst/icons@main/png/${slug}.png`

export const SERVICE_BRAND_FALLBACK = '#1d3d4e'

export const SERVICE_BRANDS: Record<ServiceKey, string> = {
  apprise: '#3B7BBF',
  arcane: '#0DB7ED',
  bytestash: '#6B73FF',
  gotify: '#45AEE5',
  linkding: '#7C5CBF',
  memos: '#3478F6',
  openai: '#10A37F',
  overseerr: '#E5870A',
  plex: '#CC7B19',
  prowlarr: '#F16529',
  qbittorrent: '#2F99E0',
  qdrant: '#DC244C',
  radarr: '#F0BC40',
  sabnzbd: '#F4A623',
  sonarr: '#35C5F4',
  tailscale: '#1E5EFF',
  tautulli: '#D9A21B',
  tei: '#FF9D00',
  unifi: '#0559C9',
  unraid: '#F45B00',
}

export const SERVICE_LOGOS: Record<ServiceKey, string | null> = {
  apprise:     selfhst('apprise'),
  arcane:      selfhst('arcane'),
  bytestash:   selfhst('bytestash'),
  gotify:      selfhst('gotify'),
  linkding:    selfhst('linkding'),
  memos:       selfhst('memos'),
  tei:         null,
  openai:      selfhst('openai'),
  overseerr:   selfhst('overseerr'),
  plex:        selfhst('plex'),
  prowlarr:    selfhst('prowlarr'),
  qbittorrent: selfhst('qbittorrent'),
  qdrant:      selfhst('qdrant'),
  radarr:      selfhst('radarr'),
  sabnzbd:     selfhst('sabnzbd'),
  sonarr:      selfhst('sonarr'),
  tailscale:   selfhst('tailscale'),
  tautulli:    selfhst('tautulli'),
  unifi:       selfhst('ubiquiti-unifi'),
  unraid:      selfhst('unraid'),
}

export const SERVICE_SVG_FALLBACKS: Partial<Record<ServiceKey, string>> = {
  apprise: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm1 15h-2v-6h2v6zm0-8h-2V7h2v2z"/></svg>`,
  arcane: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M21 4.5l-9-2.25L3 4.5v9c0 5.55 3.84 10.74 9 12 5.16-1.26 9-6.45 9-12v-9z"/></svg>`,
  bytestash: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M20 6H4V4h16v2zm0 2H4v2h16V8zm0 4H4v2h16v-2zm0 4H4v2h16v-2z"/></svg>`,
  gotify: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M20 2H4c-1.1 0-2 .9-2 2v18l4-4h14c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zm0 14H6l-2 2V4h16v12z"/></svg>`,
  linkding: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M17 7h-4v2h4c1.65 0 3 1.35 3 3s-1.35 3-3 3h-4v2h4c2.76 0 5-2.24 5-5s-2.24-5-5-5zm-6 8H7c-1.65 0-3-1.35-3-3s1.35-3 3-3h4V7H7c-2.76 0-5 2.24-5 5s2.24 5 5 5h4v-2zm-3-4h8v2H8v-2z"/></svg>`,
  memos: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M14 2H6c-1.1 0-2 .9-2 2v16c0 1.1.9 2 2 2h12c1.1 0 2-.9 2-2V8l-6-6zm2 16H8v-2h8v2zm0-4H8v-2h8v2zm-3-5V3.5L18.5 9H13z"/></svg>`,
  tei: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M21 3H3v2h9v14h2V5h9V3zM5 9v2h3v8h2v-8h3V9H5z"/></svg>`,
  openai: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M19 9l1.25-2.75L23 5l-2.75-1.25L19 1l-1.25 2.75L15 5l2.75 1.25L19 9zm-7.5.5L9 4 6.5 9.5 1 12l5.5 2.5L9 20l2.5-5.5L17 12l-5.5-2.5zM19 15l-1.25 2.75L15 19l2.75 1.25L19 23l1.25-2.75L23 19l-2.75-1.25L19 15z"/></svg>`,
  overseerr: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M12 4.5C7 4.5 2.73 7.61 1 12c1.73 4.39 6 7.5 11 7.5s9.27-3.11 11-7.5c-1.73-4.39-6-7.5-11-7.5zM12 17c-2.76 0-5-2.24-5-5s2.24-5 5-5 5 2.24 5 5-2.24 5-5 5zm0-8c-1.66 0-3 1.34-3 3s1.34 3 3 3 3-1.34 3-3-1.34-3-3-3z"/></svg>`,
  plex: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M8 5v14l11-7z"/></svg>`,
  prowlarr: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M15.5 14h-.79l-.28-.27A6.47 6.47 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z"/></svg>`,
  qbittorrent: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M19 9h-4V3H9v6H5l7 7 7-7zM5 18v2h14v-2H5z"/></svg>`,
  qdrant: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M12 3C7.58 3 4 4.79 4 7v10c0 2.21 3.58 4 8 4s8-1.79 8-4V7c0-2.21-3.58-4-8-4zm6 14c0 .63-2.13 2-6 2s-6-1.37-6-2v-1.27c1.42.82 3.6 1.27 6 1.27s4.58-.45 6-1.27V17zm0-4c0 .63-2.13 2-6 2s-6-1.37-6-2v-1.27c1.42.82 3.6 1.27 6 1.27s4.58-.45 6-1.27V13zm0-4c0 .63-2.13 2-6 2s-6-1.37-6-2 2.13-2 6-2 6 1.37 6 2z"/></svg>`,
  radarr: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M15.5 14h-.79l-.28-.27A6.47 6.47 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z"/></svg>`,
  sabnzbd: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M19 9h-4V3H9v6H5l7 7 7-7zM5 18v2h14v-2H5z"/></svg>`,
  sonarr: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm-2 14.5v-9l6 4.5-6 4.5z"/></svg>`,
  tailscale: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm-1 17.93c-3.95-.49-7-3.85-7-7.93 0-.62.08-1.21.21-1.79L9 15v1c0 1.1.9 2 2 2v1.93zm6.9-2.54c-.26-.81-1-1.39-1.9-1.39h-1v-3c0-.55-.45-1-1-1H8v-2h2c.55 0 1-.45 1-1V7h2c1.1 0 2-.9 2-2v-.41c2.93 1.19 5 4.06 5 7.41 0 2.08-.8 3.97-2.1 5.39z"/></svg>`,
  tautulli: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M3.5 18.49l6-6.01 4 4L22 6.92l-1.41-1.41-7.09 7.97-4-4L2 16.99z"/></svg>`,
  unifi: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M12 1L3 5v6c0 5.55 3.84 10.74 9 12 5.16-1.26 9-6.45 9-12V5l-9-4zm0 4l6 2.67V11c0 3.83-2.57 7.43-6 8.93C8.57 18.43 6 14.83 6 11V7.67L12 5z"/></svg>`,
  unraid: `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M19 3H5c-1.1 0-2 .9-2 2v4c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm0 6H5V5h14v4zm0 4H5c-1.1 0-2 .9-2 2v4c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2v-4c0-1.1-.9-2-2-2zm0 6H5v-4h14v4zm-9-9H8v-2h2v2zm0 8H8v-2h2v2z"/></svg>`,
}

/**
 * Maps `.env` variable prefixes to their `SERVICE_KEYS` entry.
 * Used by gateway-form-dialog to detect services from pasted env text.
 */
export const SERVICE_ENV_PREFIXES: Record<string, ServiceKey> = {
  APPRISE: 'apprise',
  ARCANE: 'arcane',
  BYTESTASH: 'bytestash',
  GOTIFY: 'gotify',
  LINKDING: 'linkding',
  MEMOS: 'memos',
  OPENAI: 'openai',
  OVERSEERR: 'overseerr',
  PLEX: 'plex',
  PROWLARR: 'prowlarr',
  QBITTORRENT: 'qbittorrent',
  QDRANT: 'qdrant',
  RADARR: 'radarr',
  SABNZBD: 'sabnzbd',
  SONARR: 'sonarr',
  TAILSCALE: 'tailscale',
  TAUTULLI: 'tautulli',
  TEI: 'tei',
  UNIFI: 'unifi',
  UNRAID: 'unraid',
}
