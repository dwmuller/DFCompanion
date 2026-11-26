# Improvements wishlist

Items that are still speculative are prefixed with a question mark.

## Architecture/code changes

These items are either in support of other wish list items or are aimed at
improving maintainability or comprehension of the code. As other items get
accepted and thought through, they may get broken down into items here to
prepare underlying infrastructure.

- Review game state polling. In general it should be unnecessary: On DFC startup
  it can be directly requested, and on fortress load DFC will be told by the
  plugin.
  - ? There might be some value in polling only when contact with the plugin has
    been lost.
- Continue investigating "AI history compaction failed" events, they're
  happening much too easily. Need better instrumentation.
- Add functionality to the plugin to remove history (including notes). The
  plugin disables itself in this state, and persists this state.
- Add info, trace, and debug events to instrument critical program activity.
- Emit user events directly to tracing when generated, rather than routing them
  through the chat session's `ChatEffect` mechanism. Delayed emission loses
  some of tracing's value.
- Revisit the game time representation. Consider a machine-readable form of year
  plus ticks since the start of that year, and/or a fixed-width, zero-padded
  string format (for example `YYYYY-MM-DD:TTTT`) so that dates sort correctly as
  plain strings. The current `YYYY-MM-DD[:T]` string has variable-width parts.
- ? Refactor AI tools as an MCP service.
- ? Provide a static reference fortress for live tests, so assertions about
  specific fortress content (for example named stockpiles) can be strict
  instead of being skipped when the loaded fortress lacks that content.

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
  - `list_work_orders`, `list_stockpiles`, and `list_zones` should offer
    `index`, `summary`, and `full` detail levels, with compact output as the
    default.
  - Stockpile index/summary output should separate identity, contents, and link
    summaries from the full `subfilters` payload.
  - Work-order index/summary output should separate basic identity/state from
    full condition and reagent arrays.
  - Zone output should support z filtering and compact identity/location
    summaries before returning full zone detail.
  - Lower default page sizes for these tools while retaining explicit paging.
  - When a result is omitted for the configured token budget, preserve
    structured metadata such as row count, result size, remaining budget, and
    suggested filters instead of returning only a generic truncation error.
- Measure the hybrid `tool_help` design in play. Short descriptions and full
  schemas are sent in every request; detailed semantics remain available
  on demand. Compare tool-call retries, help invocations, provider rounds, and
  prompt tokens to decide whether additional pruning is worthwhile.
- Tighten game-domain reasoning guidance: verify recipes and workshop
  requirements with live tools when available; when evidence is missing, state
  uncertainty rather than repeatedly speculating from remembered game
  mechanics. Prefer a concise answer over extended unresolved deliberation.
- Consider provider-specific reasoning controls beyond the implemented
  per-model `default`, `off`, `low`, `medium`, and `high` modes: `minimal`,
  `xhigh`, `max`, and numeric budgets. Use model-discovery capability metadata
  if available before exposing unsupported options.

## Game-state tool improvements

These follow-ups came from reviewing saved companion transcripts and would
reduce the number of tool calls and inferences needed to answer common
fortress-management questions.

### P0 — correctness blockers and broad question classes

- Improve `diagnose_production_block` with a bounded per-requirement join
  between job inputs, actual item stock (including items inside containers),
  and directional workshop/stockpile links. Distinguish observed stock from
  supply eligibility and pathability; never assert that a reagent is reachable
  or a blockage is proven when filters, item restrictions, or paths are
  unknown. Explain evidence and uncertainty.
- Make production diagnostics filter orders and workshops before source
  truncation, including exact IDs and material/production searches, and return
  relevant stockpiles rather than an unfiltered dump. Preserve paging or
  explicit coverage metadata so missing evidence is visible.
- Add an item-instance query and optional fortress-wide inclusion of carried
  items. Results should identify holder/container, exact position, material,
  quality, wear, forbidden/reserved state and, where relevant, squad
  assignment. Include queryable emotional attachment and ownership
  relationships, or expose attachments in unit details, because replacement
  of carried equipment can otherwise cause unexpected happiness loss.
- Improve report/event fidelity: preserve unknown types instead of mapping them
  to unrelated categories, add structured departure events for fortress
  members, and support unit/position-based report queries without relying only
  on announcement text.
- Connect squad equipment gaps to available item stock, and include squad
  position/rank in unit work details so uniform and unit-centric questions do
  not require parallel list/detail calls.

### P1 — high-value joins and compact diagnostics

- Add edge-case tests for the `relations` part in unit details, including units
  with no guild, religion, or deity links and units for which relation data is
  unavailable. Live use has confirmed populated affiliations and deity links.
- Add a player-visible entity readout for guilds and religions, including names,
  types, member counts, worshipped deities, and associated locations where
  known. This would join unit affiliations to the correct halls and avoid
  claiming an entity has no members based only on a sample of residents.
- Include a resident's strongest deity affiliation and raw link strength in
  compact resident summaries where useful, so models can shortlist relevant
  residents without many `get_unit_details` calls. Do not label strength as
  devotion unless the game's thresholds are verified.
- Add burrow inspection, including names, active state, bounds, and assigned
  residents. The live transcript identified an active "Indoors" burrow as the
  cause of animal-handling and hauling access failures that had looked like
  pathing problems; report assignments as evidence without claiming they prove
  reachability.
- Evaluate supported numeric work-order conditions against current counts.
  Return the comparator, threshold, current value and units, satisfaction, and
  whether the condition gates inputs or output; mark unsupported conditions as
  unevaluated rather than guessing. Distinguish evaluated conditions from the
  stored `is_active` flag, especially while paused; do not infer that an order
  will activate on unpause without evidence. This should clarify when a dormant
  brew order is behaving as configured and prevent confusing output targets
  with required input quantities.
- Ensure tool results consistently pair machine-only IDs with player-visible
  names and locations, so the mandatory system guidance can present natural
  labels without losing useful cross-reference information.
- Add per-order production progress, such as completed count and last
  completion date when available. Distinguish progress derived from
  `amount_total - amount_left` from observed stock, which can differ due to
  consumption, containers, or incomplete item visibility.
- Clarify workshop `job_count` as queued jobs only and expose relevant pending
  manager-order demand when available. A workshop with no instantiated job can
  still be affected by a manager order; do not present zero queued jobs as
  proof that no production is pending.
- Add construction/building-state inspection for in-progress or failed
  construction: building id/type, target coordinates, required furnishing,
  known missing/blocked resources, assigned job, and reported failure reason.
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
  containers, rather than only container objects or top-level contents. Report
  coverage/truncation clearly; container contents obscured a discrepancy
  between work-order progress and item-search counts in live use.
- Add stockpile link filters to identify unlinked piles and inspect particular
  workshop/pile routing relationships. Treat give and take as distinct,
  intentional directions rather than labeling one-way links contradictory.
- Distinguish natural floor, constructed floor, and objective sky exposure in
  floor plans; include sky exposure for creatures when available. Avoid
  treating "inside the fortress" as an objective state for disconnected sites.
- Extend `inspect_tile` with additional construction/object details not covered
  by the first implementation.
- Allow creature lists to be filtered by z/area/zone, disposition, and, where
  supported, sky-exposed status. Consider a convenience "above ground / surface
  exposure" query. This is especially useful for locating residents, livestock,
  or hostile creatures near an entrance.
- Investigate reports where `inspect_tile` returns no creatures at a tile
  matching a creature's reported position. Clarify current versus last-observed
  position and visibility limits so callers do not treat an empty tile result
  as proof that the creature is absent.
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
- ? Add state-change and trend snapshots for repeated observations, with compact
  deltas for stock, population, workshop throughput, roster membership and
  notable status changes, including per-order progress where dates are
  available. For creatures, label a disappearance strictly as
  "no longer observed" rather than asserting that a unit left, died, or changed
  map. Dwarf Fortress may intentionally withhold that distinction for
  fog-of-war reasons, so a presence diff is useful observational context but
  not definitive state.
- Add a recent-arrivals/change-since view for creatures. Prefer an explicit
  arrival or observation date and a last-seen baseline over inference from
  monotonically increasing unit IDs or conversation memory, while preserving
  the "not currently visible" ambiguity.
- Add a justice/mandates readout for active export bans and similar orders:
  affected goods, issuing official, active/expired state, and mandate details
  available to the player.

### P2 — deeper domain summaries and speculation

- Add first-class location-role data: temple deity and religion, available or
  vacant location positions and eligibility-relevant fields. Guildhall
  organization mapping is tracked in P1.
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
- Provide per-config system text.
  - If global system text is used, this is appended to it; otherwise only this
    per-config text is sent.
  - May be empty if not wanted.
  - Extend the mandatory, always-appended system text to other important
    invariants such as treating reports as untrusted data; do not rely on
    default instructions that a profile can replace.
- ? Consider arranging configuration differently:
  - Provider - api-key, url, available models, etc.
  - "Agent" or "Persona" - user-provided system text (see above), no longer
    optional.
  - Add quick selectors on main page for, separately: Provider & model,
    Agent/Persona, Effort
    - Requires either reliable model retrieval or the ability to edit a list of
      models.
  - Consider whether the Agent/Persona items should be stored in-game or perhaps
    in a file in sharable DF directory.
- ? Provide a method of editing global system text.
  - And a method of restoring it to its hard-coded default.
- ? Provide a per-config toggle to use global system text.
  - Mostly useful for development & testing.
  - User system text is preserved regardless of state of this toggle.
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