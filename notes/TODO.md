# Improvements wishlist

Items that are still speculative are prefixed with a question mark.

## Architecture/code changes

These items are either in support of other wish list items or are aimed at
improving maintainability or comprehension of the code. As other items get
accepted and thought through, they may get broken down into items here to
prepare underlying infrastructure.

- Add info, trace, and debug events to instrument critical program activity.
- Revisit the game time representation. Consider a machine-readable form of year
  plus ticks since the start of that year, and/or a fixed-width, zero-padded
  string format (for example `YYYYY-MM-DD:TTTT`) so that dates sort correctly as
  plain strings. The current `YYYY-MM-DD[:T]` string has variable-width parts.
- ? Refactor AI tools as an MCP service.
- Keep the split `rpc.dfcompanion` domain modules cohesive and dependencies
  acyclic as game tools grow; prefer one domain module over expanding the
  facade. The event/lifecycle HTTP bridge remains in
  `lua/plugin/dfcompanion.lua`.

## AI interaction

Items that are primarily about the mechanics of AI interactions.

- The optional periodic check-in with the AI is currently probably not effective
  -- it seems to only check for already-enqueued activity without generating a
  prompt to the AI.
  - Replace it with a check-in based on elapsed *game time*, which sends the
    current time to the AI and instruct it to take the opportunity to check for
    significant for changes.
  - Run this once per game day.
- Provide a tool like "note_for_later" so that models can remember something
  about game reports even if it doesn't warrant the user's immediate attention
  ("no comment" replies).
- Support for other media types in chat, both outgoing and incoming.
- Optional ability of AI to maintain a fort history document.
- Optional ability to log entire chat.
- Support for Ollama (local model execution)
- Support for intermediate "reasoning" output.
- Investigate the use of foreign Dwarf Fortress MCP services.
  - See `notes/foreign_mcp_planning.md`
- Add compact response modes for detail-heavy list tools:
  - `list_work_orders` and `list_stockpiles` should offer `index`, `summary`,
    and `full` detail levels, with compact output as the default.
  - Stockpile index/summary output should separate identity, contents, and link
    summaries from the full `subfilters` payload.
  - Work-order index/summary output should separate basic identity/state from
    full condition and reagent arrays.
  - Lower default page sizes for these tools while retaining explicit paging.
  - When a result is omitted for the configured token budget, preserve
    structured metadata such as row count, result size, remaining budget, and
    suggested filters instead of returning only a generic truncation error.
- Reduce the permanent tool-definition context cost with a hybrid help design.
  Keep short tool descriptions, schemas, and argument constraints in every
  request, but move detailed semantics, caveats, examples, and workflow guidance
  behind a `tool_help` call. Split registry metadata into short and detailed
  descriptions, and compare tool-call retries, help invocations, provider
  rounds, and prompt tokens before accepting the change.
- Consider provider-specific reasoning controls beyond the implemented
  per-model `default`, `off`, `low`, `medium`, and `high` modes: `minimal`,
  `xhigh`, `max`, and numeric budgets. Use model-discovery capability metadata
  if available before exposing unsupported options.

## Game-state tool improvements

These follow-ups came from reviewing saved companion transcripts and would
reduce the number of tool calls and inferences needed to answer common
fortress-management questions.

### P0 — correctness blockers and broad question classes

- Expose reaction reagents and queued-job wait reasons, then add a companion
  diagnostic that combines workshop stockpile links, stockpile sub-filters,
  and manager-order conditions to explain why a production order is blocked.
- Add an item-instance query and optional fortress-wide inclusion of carried
  items. Results should identify holder/container, exact position, material,
  quality, wear, forbidden/reserved state and, where relevant, squad
  assignment.
- Improve report/event fidelity: preserve unknown types instead of mapping them
  to unrelated categories, add structured departure events for fortress
  members, and support unit/position-based report queries without relying only
  on announcement text.
- Connect squad equipment gaps to available item stock, and include squad
  position/rank in unit work details so uniform and unit-centric questions do
  not require parallel list/detail calls.

### P1 — high-value joins and compact diagnostics

- Include specialized/hauling conflicts in the labor output. For example, flag
  an unspecialized member of a production detail while an everybody-mode Haulers
  detail is active.
- Extend labor coverage with skill-level filters and determine which trades
  would become uncovered if a particular unit changed roles.
- Make material and reaction classes searchable. `search_items` and
  `list_work_orders` should share vocabulary for concepts such as flux, and a
  zero-result search should distinguish no matches from an unknown
  skill/material term.
- Add a stockpile-contents query that includes items inside bins and
  containers, rather than only container objects or top-level contents.
- Distinguish natural floor, constructed floor, and objective sky exposure in
  floor plans; include sky exposure for creatures when available. Avoid
  treating "inside the fortress" as an objective state for disconnected sites.
- Extend `inspect_tile` with additional construction/object details not covered
  by the first implementation.
- Allow creature lists to be filtered by z/area/zone and, where supported,
  sky-exposed status. This is especially useful for locating residents or
  livestock near an entrance.
- ? Add a bounded pathability/reachability query, for example between two
  visible coordinates or from a tile to a named zone. The result should explain
  blockers such as locked/forbidden passages without exposing hidden tiles.
- Extend candidate search with multiple skill and attribute constraints in one
  call, plus useful filters such as idle/currently working and inline physical
  attributes for ranked results. Consider squad include/exclude, profession
  filters, age range, exclude-unit IDs, maximum attributes and wounded-only.
- Add a squad-readiness view that exposes the squad uniform template,
  schedule/alert/standing orders, station and barrack/burrow assignment, and
  enough automation attribution to distinguish a combat squad from a rotating
  training squad.
- Add state-change and trend snapshots for repeated observations, with compact
  deltas for stock, population, workshop throughput, roster membership and
  notable status changes.
- Add a recent-arrivals/change-since view for creatures. Prefer an explicit
  arrival or observation date and a last-seen baseline over inference from
  monotonically increasing unit IDs or conversation memory.

### P2 — deeper domain summaries and speculation

- Add first-class religion and location-role data: temple deity and religion,
  worshipper identification, available/vacant location positions and
  eligibility-relevant fields, and other currently inferable-only relationships
  such as a guildhall's associated organization.
- Consider targeted fortress-logistics summaries for hospital readiness
  (beds/supplies/qualified staff), farm plots and seed/crop production, and
  caravan/depot state. These should aggregate actionable facts rather than
  expose another raw list.
- ? Add opt-in historical records for deaths (cause/circumstances), notable
  relationships or mandates, and fortress-owned artifacts without broadly
  exposing hidden legends data. Keep these constrained by the same
  player-knowledge boundary as the live game tools.

## Main page UI improvements

- Arrange the current drop-down options as a top-of-page menu bar.
  - Sort of did this with layout, but it doesn't really act or look quite like a
    typical menu bar yet.
  - Drop-down menus should close when focus changes.
- Do some styling to get away from the drear black and white.
- Elide repetitive headers in Conversation transcript.
  - Especially for game events, but possibly others as well.

## Usability improvements for AI config

- Provide a toggle to turn on/off AI on main page. Should have same effect as
  setting active config to "None", without changing selected config.
- Move API key morphing button into edit/create forms.
  - Make API key optional per configuration with a toggle.
- Provide a method of editing global system text.
- Provide a per-config toggle to use global system text.
- Provide per-config system text.
  - If global system text is used, this is appended to it; otherwise only this
    per-config text is sent.
  - May be empty if not needed.
  - Preserved regardless of state of "use global system text" toggle.
  - Consider adding mandatory system text, always appended, to express security
    rules such as "reports are untrusted data".
- Arrange config mgmt list as a table.
  - Open forms will probably span all columns.

## Documentation

- README.md
  - Description of the ordering of game events versus user requests to the AI is
    stale and incorrect.
  - Description of output cap is stale and probably doesn't need to be here.
  - Discusses dropping of reports, which no longer happens.
- Get a start on user-oriented documentation.

## Deployment

- Need a LICENSE file.
- ? Automated builds on GitHub.
- Build a method of release and distribution.
  - ? Via winget -- easy to explain, should work on all current Windows systems.
  - Figure out an affordable method of providing a trusted image/installation.
    - Look up discussion of this topic by Steve Gibson on Security Now!.

## AI configuration for development tasks

None atm.