# 0017: Transcript blocks

- Status: landed
- Author: r105
- Scope: `src/ui/transcript.rs`, `src/ui/commands.rs`, `src/ui/render.rs`, `src/ui/mod.rs`, `src/command.rs`

## Goal

Every transcript message is an addressable **block** (number = 1-based
history position, rendered as `#n` on non-section headers). Blocks can be
filtered, copied, and rerun, implemented on r105's message model. Shell
`!cmd` and its output notice stay separate adjacent messages, which
block addressing makes unambiguous.

## Behavior

- Gutter: user/assistant/system headers render `USER #3` etc. Tool and
  thinking sections keep their `[n]` section gutters for `/expand`, so
  the two numbering spaces never collide. Block addresses accept the
  rendered `#n` form or a bare number in every command below.
- `/filter <block> <pattern> [--regex] [--case] [--invert] [--context N]`
  stores a filter on that block. Bare `/filter <block>` or `--clear`
  removes it; bare `/filter` lists active filters. Matching is
  case-insensitive substring by default; `--case` makes it sensitive,
  `--regex` compiles a regex (bad pattern = error, nothing stored),
  `--invert` keeps non-matching lines, `--context N` keeps N lines
  around each hit (default 0).
- Filtered render: matching lines (+ context) replace the body, followed
  by a dim trailer `⋯ {hidden} line(s) hidden · /filter {n} --clear`.
  Collapsed tool sections preview the first *matching* line. Filters are
  keyed by message id in `block_filters` and pruned with the section
  overrides; they are view-local (no session persistence).
- `/block [n]`: no arg lists the last 40 blocks (`n · role · preview ·
  ·filtered`); with `n` shows role, size, filter state, and first line.
- `/copy out [n]`: copies block `n` (default: last message) via the
  existing clipboard path — the filtered view when a filter is set,
  otherwise verbatim. Plain `/copy [n]` keeps code-block semantics.
- `/rerun [n]`: re-submits block `n` (default: last user message) as if
  freshly entered. Prompts and `!` shell lines run again immediately;
  `/` commands prefill the composer instead, because replaying `/clear`
  or `/exit` is not what rerun should mean. Refuses while busy and on
  non-user blocks.
- `/expand` accepts `#n` block addresses in addition to `[n]` section
  numbers; a block that is not a collapsible section reports so.

## State

- `UiApp.block_filters: HashMap<String, BlockFilter>` (message id →
  filter).
- `BlockFilter { pattern, regex, case, invert, context }` with
  `describe()`, plus pure `parse_block_filter(history_len, args) ->
  Result<FilterAction, String>` and `apply_block_filter(content, filter)
  -> FilteredLines { shown, hidden }`.
- `rerun_target(history, arg) -> Result<String, String>` — pure target
  resolution.

## Tests

- Filter parsing: flags, `#n` form, clear, bad regex, missing context
  value, out-of-range block, empty history.
- Apply: substring/case/invert/context/regex, hidden counts, all-hidden.
- `/filter` stores by id and clears; `/block` reports it.
- `/rerun` target resolution: default last user turn, explicit n,
  non-user block rejected, out-of-range rejected, empty history.
- `/expand #n` maps to sections; failed/non-section blocks refuse.
- Render: `#n` gutters appear; `/help` groups cover the new commands.
