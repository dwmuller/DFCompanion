local _ENV = mkmodule('rpc.dfcompanion')

-- Thin RunLua-facing facade. Domain implementation lives in rpc/dfcompanion/
-- modules so callers continue to use the stable rpc.dfcompanion module name.
local modules = {
    lifecycle = require('dfcompanion.internal.lifecycle'),
    creatures = require('dfcompanion.internal.creatures'),
    work = require('dfcompanion.internal.work'),
    reports = require('dfcompanion.internal.reports'),
    items = require('dfcompanion.internal.items'),
    places = require('dfcompanion.internal.places'),
    squads = require('dfcompanion.internal.squads'),
    moods = require('dfcompanion.internal.moods'),
    automation = require('dfcompanion.internal.automation'),
    tile = require('dfcompanion.internal.tile'),
}

local exports = {
    pause_for_ai_block = modules.lifecycle,
    check_session = modules.lifecycle,
    take_ai_block_pause_reason = modules.lifecycle,
    z_offset = modules.lifecycle,
    game_date = modules.lifecycle,
    fortress_name = modules.lifecycle,
    load_chat_state = modules.lifecycle,
    save_chat_state = modules.lifecycle,
    lifecycle_state = modules.lifecycle,
    script_enabled = modules.lifecycle,
    known_creatures = modules.creatures,
    find_candidates = modules.creatures,
    mind_of = modules.creatures,
    list_work_details = modules.work,
    labor_coverage = modules.work,
    list_work_orders = modules.work,
    get_reaction = modules.work,
    report_entry = modules.reports,
    recent_reports = modules.reports,
    search_items = modules.items,
    food_drink_summary = modules.items,
    housing = modules.places,
    list_zones = modules.places,
    list_locations = modules.places,
    list_workshops = modules.places,
    building_counts = modules.places,
    geometry_overview = modules.places,
    geometry_places = modules.places,
    list_stockpiles = modules.places,
    list_squads = modules.squads,
    strange_moods = modules.moods,
    dfhack_automation = modules.automation,
    inspect_tile = modules.tile,
}

-- Every export takes the expected fortress session as its first argument and
-- fails if it is no longer current, unless it is listed here. New exports are
-- therefore guarded by default.
local session_exempt = {
    -- Session bootstrap: DFC learns the session from this call.
    lifecycle_state = true,
    -- These check the session themselves.
    load_chat_state = true,
    save_chat_state = true,
    -- Used in-process by the plugin, not by DFC over RPC.
    game_date = true,
    fortress_name = true,
    z_offset = true,
    script_enabled = true,
    report_entry = true,
    take_ai_block_pause_reason = true,
}

for name, module in pairs(exports) do
    local implementation = module[name]
    if implementation == nil then
        error('Missing rpc.dfcompanion export: ' .. name)
    end
    if not session_exempt[name] then
        local unguarded = implementation
        implementation = function(expected_session, ...)
            modules.lifecycle.require_session(expected_session)
            return unguarded(...)
        end
    end
    _ENV[name] = implementation
end

-- A lifecycle poll also establishes contact with the companion service.
function lifecycle_state()
    local state = modules.lifecycle.lifecycle_state()
    reqscript('dfcompanion').onCompanionContact()
    return state
end

return _ENV
