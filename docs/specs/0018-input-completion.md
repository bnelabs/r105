# 0018: Input history walk and completion engine

- Status: landed
- Author: r105
- Scope: `src/suggest.rs`, `src/ui/input.rs`, `src/ui/ghost.rs`, `src/ui/complete.rs`, `src/ui/render.rs`, `src/ui/mod.rs`

## Goal

Composer history works like a terminal (preview + restore), popups obey
a single-owner rule, the empty composer teaches its own shortcuts, and
the ghost cascade gains a command-name layer, adapted to r105's
Tab/ghost/mode chain (unchanged).

## Behavior

- ↑/↓ walk user messages: first ↑ stashes the live draft, ↑/↓ move
  through older/newer user turns, ↓ past the newest restores the stash.
  Depth clamps at both ends. Any real edit (type, backspace, delete,
  Tab/ghost accept) or submit adopts the preview as the new draft and
  ends the walk. Esc during a walk restores the stash and ends the walk
  before ghost/cancel handling. Walks cover prompts, `!` lines, and
  dispatched `/` commands — whatever history stores with role `user`.
- Single owner: the ghost tick clears and suppresses itself while a menu
  wants input. `menu_wants_input()` is true for an open overlay, the
  slash palette, a live `@path` token, or a slash-command argument
  position with candidates. Cheap enough for the 45ms tick (the arg
  menu uses its existing input-keyed cache; no filesystem work).
- Empty-composer hint (display only, never part of the input): dim text
  after `> ` — busy: `working… Enter queues · Ctrl+X cancels`; else
  `prompt · ! shell · / commands · # route · ↑ history`. An active walk
  retitles the composer `↑↓ history · Esc restore`.
- Command-name layer in `suggest()`: for a shell line whose first token
  has no whitespace yet, complete against shell builtins (static list)
  + PATH executables (cached 30s in `UiApp.bin_cache`). Cascade order:
  history frequency → command name → path top-hit. Single best suffix
  flows through the existing ghost channel (Tab accepts, typing
  narrows); no new popup. The ghost is append-only, so only prefix
  matches qualify; shorter names win, ties keep the builtin.
- `suggest()` gains a `bins: &[String]` parameter; `scan_path_bins()`
  reads PATH, `scan_bins(dirs)` owns the fixture-testable scan (files
  only, capped at 2000, sorted, deduped).

## State

- `UiApp.hist_depth: Option<usize>` (1 = most recent user turn),
  `UiApp.draft_stash: String`.
- `UiApp.bin_cache: Vec<String>`, `UiApp.bin_cache_at: Option<Instant>`.

## Non-goals

- Flag/subcommand signature registries (a static builtin list + PATH
  covers the common cases; signatures are a later spec).
- Autodetect routing (Batch 3); `#` routing stays as-is.

## Tests

- Walk: stash on first ↑, clamp at oldest, ↓ restores stash, edit
  adopts, Esc restores.
- `menu_wants_input`: slash prefix, `@token`, arg position, negatives;
  `tick_ghost` clears the ghost when a menu opens.
- `command_guess`: builtin hit, PATH-bin hit, argument territory,
  exact-match no-op; cascade order history → bins → path.
- `scan_bins`: sorted, deduped, directories skipped (fixture PATH).
- Empty-composer hint renders; walk retitles the composer.
