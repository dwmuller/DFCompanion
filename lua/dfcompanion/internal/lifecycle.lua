local common = require('dfcompanion.internal.common')
local json = common.json
local ui_z = common.ui_z
local z_offset = common.z_offset

local ai_block_pause_reason
local CHAT_STATE_KEY = 'dfcompanion/chat'
local SAVED_STATE_VERSION = 2
local MAX_ENCODED_CHAT_BYTES = 2 * 1024 * 1024

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

local function read_saved_state()
    local raw = dfhack.persistent.getSiteDataString(CHAT_STATE_KEY)
    if not raw then return nil end
    local state = json.decode(raw)
    if type(state) ~= 'table' then
        error('Saved companion state is not a table.')
    end
    if state.enabled ~= nil and type(state.enabled) ~= 'boolean' then
        error('Saved companion state has an invalid enabled flag.')
    end
    if state.version == 1 then
        return state, raw
    end
    if state.version ~= SAVED_STATE_VERSION then
        error('Saved companion state has an unsupported version: ' .. tostring(state.version))
    end
    if state.chat_history == nil and state.enabled ~= nil then
        return state
    end
    if type(state.chat_history) ~= 'string' or state.chat_history == '' then
        error('Saved companion state has invalid chat history.')
    end
    if #state.chat_history > MAX_ENCODED_CHAT_BYTES then
        error('Saved encoded chat state exceeds the supported size limit.')
    end
    return state
end

local function load_chat_state(expected_site_id)
    require_chat_site(expected_site_id)
    local state, legacy = read_saved_state()
    if not state then return '' end
    if legacy then
        if state.enabled == nil then return 'json:' .. legacy end
        state.enabled = nil
        return 'json:' .. json.encode(state, {pretty = false})
    end
    if state.chat_history == nil then return '' end
    return 'zstd:' .. state.chat_history
end

local function load_enabled_state()
    require_chat_site()
    local saved = read_saved_state()
    if not saved then return nil end
    if saved.enabled == nil then return true end
    return saved.enabled
end

local function save_enabled_state(enabled)
    require_chat_site()
    if type(enabled) ~= 'boolean' then
        error('Companion enabled state must be a boolean.')
    end
    local saved = read_saved_state() or {version = SAVED_STATE_VERSION}
    saved.enabled = enabled
    dfhack.persistent.saveSiteData(CHAT_STATE_KEY, saved)
end

local function save_chat_state(expected_site_id, state)
    require_chat_site(expected_site_id)
    if type(state) ~= 'string' or state:sub(1, 5) ~= 'zstd:' then
        error('Chat state must use the zstd encoding.')
    end
    state = state:sub(6)
    if state == '' then error('Encoded chat state must not be empty.') end
    if #state > MAX_ENCODED_CHAT_BYTES then
        error('Encoded chat state exceeds the supported size limit.')
    end
    local saved, legacy = read_saved_state()
    if not saved or legacy then
        saved = {version = SAVED_STATE_VERSION, enabled = saved and saved.enabled}
    end
    saved.chat_history = state
    dfhack.persistent.saveSiteData(CHAT_STATE_KEY, saved)
    return 'true'
end

local function lifecycle_state()
    local map_loaded = dfhack.isMapLoaded()
        and dfhack.world.isFortressMode()
        and dfhack.isSiteLoaded()
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
    load_enabled_state = load_enabled_state,
    save_enabled_state = save_enabled_state,
    lifecycle_state = lifecycle_state,
    script_enabled = script_enabled,
}
