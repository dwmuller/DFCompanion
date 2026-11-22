local common = require('dfcompanion.internal.common')
local json = common.json
local ui_z = common.ui_z
local z_offset = common.z_offset

local ai_block_pause_reason
local CHAT_STATE_KEY = 'dfcompanion/chat'
local MAX_CHAT_STATE_BYTES = 1024 * 1024

local function pause_for_ai_block(reason)
    if not dfhack.world.isFortressMode() or not dfhack.isSiteLoaded() then
        error('Cannot pause Dwarf Fortress because no fortress is loaded.')
    end
    if df.global.pause_state then
        return 'false'
    end
    local previous_reason = ai_block_pause_reason
    ai_block_pause_reason = tostring(reason or '')
    local ok, err = pcall(function() dfhack.world.SetPauseState(true) end)
    if not ok then
        ai_block_pause_reason = previous_reason
        error(err)
    end
    return 'true'
end

local function take_ai_block_pause_reason()
    local reason = ai_block_pause_reason
    ai_block_pause_reason = nil
    return reason or ''
end

local function game_date()
    return common.format_date(df.global.cur_year, df.global.cur_year_tick)
end

local function fortress_name()
    local site = dfhack.world.getCurrentSite()
    if not site then
        return ''
    end
    local native = dfhack.translation.translateName(site.name, false)
    local english = dfhack.translation.translateName(site.name, true)
    if native == '' or native == english then
        return english
    end
    return native .. ', "' .. english .. '"'
end

local function require_chat_site(expected_site_id)
    if not dfhack.world.isFortressMode() or not dfhack.isSiteLoaded() then
        error('Cannot access saved chat state because no fortress site is loaded.')
    end
    local site = dfhack.world.getCurrentSite()
    if not site then
        error('Cannot identify the loaded fortress site.')
    end
    if expected_site_id ~= nil and tostring(site.id) ~= tostring(expected_site_id) then
        error('The loaded fortress site changed before chat state could be accessed.')
    end
    return site
end

local function load_chat_state(expected_site_id)
    require_chat_site(expected_site_id)
    return dfhack.persistent.getSiteDataString(CHAT_STATE_KEY) or ''
end

local function save_chat_state(expected_site_id, state)
    require_chat_site(expected_site_id)
    state = tostring(state or '')
    if #state > MAX_CHAT_STATE_BYTES then
        error('Chat state exceeds the supported size limit.')
    end
    dfhack.persistent.saveSiteDataString(CHAT_STATE_KEY, state)
    return 'true'
end

local function lifecycle_state()
    local map_loaded = dfhack.isMapLoaded()
    local name = ''
    local site_id = nil
    if map_loaded then
        name = fortress_name()
        site_id = require_chat_site().id
    end
    return json.encode({
        world_loaded = dfhack.isWorldLoaded(),
        map_loaded = map_loaded,
        fortress_name = name,
        site_id = site_id,
    })
end

local function script_enabled()
    local output = dfhack.run_command_silent('enable')
    local state = output:match('dfcompanion:%s+(%S+)')
    return tostring(state == 'on')
end

return {
    pause_for_ai_block = pause_for_ai_block,
    take_ai_block_pause_reason = take_ai_block_pause_reason,
    z_offset = z_offset,
    game_date = game_date,
    fortress_name = fortress_name,
    load_chat_state = load_chat_state,
    save_chat_state = save_chat_state,
    lifecycle_state = lifecycle_state,
    script_enabled = script_enabled,
}
