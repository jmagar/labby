# Changelog

All notable changes to the `quick-push` skill are recorded here. Format roughly follows [Keep a Changelog](https://keepachangelog.com/).

## [Unreleased]
- Move session capture before staging so the generated `save-to-md` document can be committed with the quick-push changes.
- Clarify repo-root staging, ignored session-doc detection with `git check-ignore`, and force-adding ignored session docs.
- Constrain quick-push session capture to documentation-focused work so broad maintenance mutations are recorded as follow-up work instead of being swept into the commit.
- Remove stale guidance about amending the current push's changelog entry after commit.

## [0.1.1] - 2026-05-17
- Concretized step 2.7 version-sync verification: replaced hand-wavy "search for stale old-version references" with a concrete `git grep -F "<old_version>"` command across common manifest/doc extensions.
- Clarified step 3 / step 4 sequencing used by this historical version.
- Added README.

## [0.1.0] - Initial
- Initial skill version.
