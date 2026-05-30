# Compose Expert Skill

Compose and Compose Multiplatform expert for UI development across Android, Desktop, iOS, and Web. Use whenever the user mentions Compose APIs (@Composable, remember, LaunchedEffect, NavHost, MaterialTheme, LazyColumn, Modifier, recomposition), Compose Multiplatform (commonMain, expect/actual, Res.*, ComposeUIViewController, UIKitView, ComposeViewport), Android TV (tv-material, D-pad, focus, Carousel), Material 3 motion, atomic design systems, design-to-code workflows, Paging 3, or navigation. Activates Review Mode on GitHub PR URLs and review phrases ("review this PR", "what's wrong with this"). Auto-detects Compose projects on session_start. Backed by actual androidx/androidx and JetBrains/compose-multiplatform-core source receipts. See "## When this skill applies" in SKILL.md for the full trigger surface.

## Usage

Invoke this skill when the user request matches the trigger conditions in `SKILL.md`. The skill body is the source of truth for workflow steps and operational constraints.

## Files

- `SKILL.md` - agent workflow and trigger guidance
- `agents/` - OpenAI runtime metadata
- `references/` - progressively loaded reference material
- `README.md` - packaging overview
- `CHANGELOG.md` - packaging change history
