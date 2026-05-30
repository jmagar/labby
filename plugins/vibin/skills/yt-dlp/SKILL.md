---
name: yt-dlp
description: Use when the user asks to download, archive, search, inspect, or organize media with yt-dlp, MeTube, YouTube, playlists, channels, supported video sites, audio extraction, metadata, thumbnails, subtitles, Plex yt-dlp libraries, or NAS media downloads.
---

# yt-dlp

Use this skill for the whole yt-dlp domain: ad hoc video downloads, playlists, channel URLs, audio-only downloads, search/preview workflows, metadata preservation, MeTube/NAS routing, and local CLI fallback.

## Policy

- Download only content the user is authorized to download, such as their own uploads, public-domain material, Creative Commons/licensed content, or content they otherwise have rights to keep.
- Prefer the NAS/MeTube path for ad hoc downloads that should land in Plex.
- Default to audio-only downloads. Use video mode only when the user asks for video, visual media, Plex video library, or explicitly passes `--video`. Use both mode when the user asks to grab both audio and video or explicitly passes `--both`.
- Use local `yt-dlp` only when the user asks for local output, debugging, inspection, search, or a destination outside the NAS.
- Prefer preview and confirmation before downloading ambiguous searches, full albums, large channels, very large playlists, or unofficial uploads.
- Do not invent metadata. Preserve extracted metadata and sidecars; report missing fields instead of guessing.
- Never overwrite existing media. Keep archive files enabled so reruns skip already downloaded items.

## Default Routing

### NAS / Plex downloads

Default for explicit URLs unless the user says local or video:

```text
MeTube: https://metube.tootie.tv
Audio output: /mnt/user/data/media/yt-dlp-audio
Video output: /mnt/user/data/media/yt-dlp
Video Plex library: yt-dlp
Video Plex path: /data/yt-dlp
```

Use the slash command when possible:

```text
/yt-dlp <url> [url ...]
/yt-dlp --video <url> [url ...]
/yt-dlp --both <url> [url ...]
```

MeTube is deployed on `tootie` and writes into the Plex-visible media share. SWAG exposes the UI at `https://metube.tootie.tv`.

NAS `--both` runs `yt-dlp` directly inside the `metube` container over SSH instead of submitting two MeTube queue jobs. This keeps audio and video archive state separate so one format does not cause the other to be skipped by MeTube's global archive.

### Local downloads

Use local mode for filesystem-local jobs:

```text
/yt-dlp --local <url> [url ...]
/yt-dlp --local --video <url> [url ...]
/yt-dlp --local --both <url> [url ...]
```

Local defaults:

```text
audio dir: $PWD/downloads/yt-dlp-audio
video dir: $PWD/downloads/yt-dlp
archive: $PWD/downloads/.archive.txt
both-mode audio archive: $PWD/downloads/.archive-audio.txt
both-mode video archive: $PWD/downloads/.archive-video.txt
audio format: m4a
video format: bestvideo*+bestaudio/best
```

Environment overrides:

```text
METUBE_URL
METUBE_BOTH_ROUTE
METUBE_CONTAINER
METUBE_QUALITY
METUBE_FORMAT
METUBE_FORMAT_SELECTOR
METUBE_AUDIO_FORMAT
METUBE_AUDIO_ARCHIVE
METUBE_VIDEO_ARCHIVE
YT_DLP_NAS_HOST
YT_DLP_DOWNLOAD_DIR
YT_DLP_ARCHIVE
YT_DLP_VIDEO_ARCHIVE
YT_DLP_FORMAT
YT_DLP_OUTPUT_TEMPLATE
YT_DLP_AUDIO_DOWNLOAD_DIR
YT_DLP_AUDIO_ARCHIVE
YT_DLP_AUDIO_FORMAT
YT_DLP_AUDIO_OUTPUT_TEMPLATE
```

## Quality and Metadata Defaults

For local video downloads, use best available quality and preserve practical metadata:

```bash
yt-dlp \
  --format "bestvideo*+bestaudio/best" \
  --download-archive "$archive_file" \
  --output "$output_template" \
  --embed-metadata \
  --embed-thumbnail \
  --convert-thumbnails jpg \
  --write-info-json \
  --write-thumbnail \
  --write-description \
  --write-subs \
  --write-auto-subs \
  --sub-langs "all,-live_chat" \
  --embed-subs \
  --restrict-filenames \
  --yes-playlist \
  --no-overwrites \
  --continue \
  --merge-output-format mkv
```

Notes:
- `bestvideo*+bestaudio/best` chooses the best separate video/audio streams when available, falling back to the best combined stream.
- `mkv` is the safest merge container for mixed codecs and embedded subtitles. Use MP4 only when the user explicitly needs MP4 compatibility.
- Subtitles and auto subtitles are downloaded when available. Some sites do not expose usable subtitles.
- Sidecars (`.info.json`, thumbnails, descriptions) are intentional; they preserve durable metadata for later indexing or retagging.
- Archive files are intentionally global to the workflow, not per playlist folder. This prevents reruns from duplicating items after output templates or playlist folders change.

For audio-only local downloads, use `--extract-audio --audio-format m4a` by default. Use `opus` for efficient/loss-minimizing YouTube audio; use `mp3` only for compatibility.

## Search and Preview

For search requests, do not download immediately. Return candidates first:

```bash
yt-dlp "ytsearch10:<query>" --flat-playlist --print "%(title)s | %(channel)s | %(duration_string)s | %(webpage_url)s"
```

For explicit URLs that need inspection:

```bash
yt-dlp --dump-single-json --flat-playlist "<url>"
```

## Site Support

`yt-dlp --list-extractors` is the local source of truth for this installation. The official supported-sites list is generated from yt-dlp extractors, but site support changes as websites change. If a site is not listed, it may still work through the generic extractor or embedded media URLs.

Common supported families include YouTube, Vimeo, SoundCloud, Bandcamp, Twitch, TikTok, Instagram, Facebook, Twitter/X, Reddit-hosted media, archive.org, Apple Podcasts, Audius, ARD/Arte/BBC-style broadcasters, and many regional TV/news sites. DRM-protected services generally do not work.

## Troubleshooting

- `Requested format is not available`: inspect with `yt-dlp -F "<url>"`, then set `YT_DLP_FORMAT`.
- YouTube throttling or challenge errors: verify `yt-dlp --version`; cookies or PO-token handling may be needed for restricted content.
- Missing `ffmpeg`: quality merging, metadata embedding, thumbnails, subtitles, and audio extraction may fail.
- Duplicate skips: check the active `--download-archive` file before assuming a URL failed.
- MeTube queue issues: check `https://metube.tootie.tv/history` or the MeTube UI, then inspect the `metube` container logs on `tootie`.

## Response Shape

When downloading, report:

- route used: MeTube/NAS or local
- media mode: video or audio
- source URL(s)
- destination
- archive file
- command/API outcome
- any skipped, failed, or metadata-limited items
