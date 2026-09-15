# 0002: Palette recency priority and live-state badges

- Status: proposed
- Author: r105
- Scope: `src/ui.rs` (`UiApp::palette_items`, accept paths)

## Problem

The palette sorts by fuzzy score only, so a command used ten times today
ranks no better than an untouched one. Stateful rows (`/theme`, `/model`,
…) also hide their current value, forcing a round trip to see it.

## Proposal

- `UiApp` gains `recent_commands: VecDeque<String>` (cap 8, most recent
  first), recorded whenever a slash command is accepted from the palette
  or dispatched. `UiApp::palette_items` re-sorts stably so recent names
  come first; `command::palette_items` keeps its pure fuzzy order and
  stays the inner scorer (fuzzy tiers already encode exact > prefix >
  substring).
- Descriptions of stateful rows gain a live badge: `/theme`, `/model`,
  `/quality`, `/profile`, `/reasoning`, `/mode` show `· now <value>`
  taken from live state. Unknown values render no badge rather than a
  wrong one.

## Non-goals

No pinning API, no usage counts or decay, no badge on custom commands.

## Acceptance

- `palette_recency_boosts_repeated_command`
- `palette_shows_live_theme_badge`
- Existing `command.rs` palette tests keep passing unchanged.

## Amendments

- Mode has no `/mode` command, so the badge lands on the trio instead:
  whichever of `/plan`, `/build`, `/ask` matches the live mode shows
  `· active`; the other two stay plain.
