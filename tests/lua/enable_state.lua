local root, installed_root = ...
assert(root)
local lifecycle_path = installed_root
    and installed_root .. '\\hack\\lua\\dfcompanion\\internal\\lifecycle.lua'
    or root .. '\\lua\\dfcompanion\\internal\\lifecycle.lua'
local plugin_path = installed_root
    and installed_root .. '\\hack\\scripts\\dfcompanion.lua'
    or root .. '\\lua\\plugin\\dfcompanion.lua'
local json = require('json')
local raw_by_site = {}
local current_site_id = 1
local site_loaded = true
local random_value = 0
local scheduled = {}
local fake_dfhack = {
    random = {new = function()
        return {random = function()
            random_value = random_value + 1
            return random_value
        end}
    end},
    timeout = function(_, _, callback)
        table.insert(scheduled, callback)
        return #scheduled
    end,
    world = {
        isFortressMode = function() return site_loaded end,
        getCurrentSite = function() return {id = current_site_id, name = {}} end,
    },
    isSiteLoaded = function() return site_loaded end,
    isMapLoaded = function() return site_loaded end,
    isWorldLoaded = function() return site_loaded end,
    translation = {translateName = function() return 'Test fortress' end},
    persistent = {
        getSiteDataString = function() return raw_by_site[current_site_id] end,
        saveSiteData = function(_, state)
            raw_by_site[current_site_id] = json.encode(state, {pretty = false})
        end,
    },
    onStateChange = {},
    printerr = function() end,
}

local function load_with_env(path, env)
    setmetatable(env, {__index = _G})
    return assert(loadfile(path, 't', env))()
end

local lifecycle = load_with_env(lifecycle_path, {
    dfhack = fake_dfhack,
    require = function(name)
        assert(name == 'dfcompanion.internal.common')
        return {json = json, z_offset = function() end}
    end,
})

local session = assert(lifecycle.current_session())
assert(not pcall(lifecycle.load_chat_state, 'stale'))
assert(not pcall(lifecycle.load_chat_state))
assert(not pcall(lifecycle.save_chat_state, 'stale', 'zstd:no'))
assert(raw_by_site[1] == nil)
assert(lifecycle.load_enabled_state() == nil)
lifecycle.save_enabled_state(false)
assert(lifecycle.load_enabled_state() == false)
assert(lifecycle.load_chat_state(session) == '')
assert(lifecycle.save_chat_state(session, 'zstd:abc') == 'true')
assert(lifecycle.load_enabled_state() == false)
assert(lifecycle.load_chat_state(session) == 'zstd:abc')
lifecycle.save_enabled_state(true)
assert(lifecycle.load_chat_state(session) == 'zstd:abc')

current_site_id = 2
raw_by_site[2] = '{"version":1,"site_id":2,"game_date":"250-01-01:0","summary":"","messages":[]}'
assert(lifecycle.load_enabled_state() == true)
lifecycle.save_enabled_state(false)
local legacy = json.decode(lifecycle.load_chat_state(session):sub(6))
assert(legacy.enabled == nil and #legacy.messages == 0)
lifecycle.save_chat_state(session, 'zstd:xyz')
assert(lifecycle.load_enabled_state() == false)
assert(lifecycle.load_chat_state(session) == 'zstd:xyz')

current_site_id = 3
raw_by_site[3] = '{"version":2,"enabled":"false","chat_history":"abc"}'
assert(not pcall(lifecycle.load_enabled_state))
assert(not pcall(lifecycle.save_enabled_state, true))
assert(raw_by_site[3] == '{"version":2,"enabled":"false","chat_history":"abc"}')
raw_by_site[3] = '{"version":1,"enabled":"false","messages":[]}'
assert(not pcall(lifecycle.load_enabled_state))
assert(not pcall(lifecycle.save_enabled_state, true))
assert(raw_by_site[3] == '{"version":1,"enabled":"false","messages":[]}')

current_site_id = 5
raw_by_site[5] = '{"version":2,"chat_history":"existing"}'
assert(lifecycle.load_enabled_state() == true)
lifecycle.save_enabled_state(true)
assert(lifecycle.load_chat_state(session) == 'zstd:existing')

local eventful = {
    eventType = {REPORT = 1},
    onReport = {},
    enableEvent = function() end,
}
local rpc = {
    game_date = function() return '250-01-01:0' end,
    take_ai_block_pause_reason = function() return '' end,
    report_entry = function() return {text = 'report'} end,
}
local service_available = false
local sent_events = {}
local sent_sessions = {}
local response_session
local function last_status()
    local last = sent_events[#sent_events]
    return last and json.decode(last).text or ''
end
local socket = {tcp = {connect = function()
    if not service_available then error('service unavailable') end
    local replies
    return {
        setTimeout = function() end,
        setBlocking = function() end,
        send = function(_, request)
            local session = assert(request:match('\r\nx%-dfcompanion%-session: ([^\r]+)'))
            table.insert(sent_sessions, session)
            local body = '{"status":"accepted"}'
            replies = {'HTTP/1.1 200 OK', 'x-dfcompanion-session: ' .. (response_session or session),
                'Content-Length: ' .. #body, '', body}
            if request:find('/api/plugin/events', 1, true) then
                table.insert(sent_events, request:match('\r\n\r\n(.*)$'))
            end
        end,
        receive = function(_, mode)
            if mode == '*l' then return table.remove(replies, 1) end
            local body = replies[1]
            local byte = body:sub(1, 1)
            replies[1] = body:sub(2)
            return byte
        end,
        close = function() end,
    }
end}}
fake_dfhack.getOSType = function() return 'windows' end
local paused = false
fake_dfhack.world.SetPauseState = function(value) paused = value end
local env = {
    dfhack = fake_dfhack,
    df = {report = {find = function()
        return {flags = {announcement = true}, text = 'report'}
    end}},
    dfhack_flags = {module = true},
    SC_MAP_LOADED = 1,
    SC_MAP_UNLOADED = 2,
    SC_WORLD_LOADED = 3,
    SC_WORLD_UNLOADED = 4,
    SC_PAUSED = 5,
    SC_UNPAUSED = 6,
    require = function(name)
        if name == 'json' then return json end
        if name == 'plugins.luasocket' then return socket end
        if name == 'plugins.eventful' then return eventful end
        if name == 'rpc.dfcompanion' then return rpc end
        if name == 'dfcompanion.internal.lifecycle' then return lifecycle end
        error('Unexpected module: ' .. name)
    end,
}
local function run_plugin()
    load_with_env(plugin_path, env)
end

site_loaded = false
run_plugin()
assert(env.enabled == nil and env.isEnabled() == false)
site_loaded = true
current_site_id = 4
fake_dfhack.onStateChange.dfcompanion_lifecycle(env.SC_MAP_LOADED)
local first_session = assert(lifecycle.current_session())
local reloaded_lifecycle = load_with_env(lifecycle_path, {
    dfhack = fake_dfhack,
    require = function(name)
        assert(name == 'dfcompanion.internal.common')
        return {json = json, z_offset = function() end}
    end,
})
assert(reloaded_lifecycle.current_session() == first_session)
assert(env.enabled == nil and not env.isEnabled() and eventful.onReport.dfcompanion)
assert(lifecycle.load_enabled_state() == nil and raw_by_site[4] == nil)
eventful.onReport.dfcompanion(1)
assert(env.enabled == nil and env.pending_event == nil and raw_by_site[4] == nil)
service_available = true
eventful.onReport.dfcompanion(1)
assert(env.isEnabled() and lifecycle.load_enabled_state() == true)
assert(sent_sessions[#sent_sessions] == first_session)
assert(last_status():find('is enabled', 1, true))

response_session = 'other'
eventful.onReport.dfcompanion(1)
assert(paused and env.pending_event and #scheduled > 0)
response_session = nil
while #scheduled > 0 do table.remove(scheduled, 1)() end
assert(env.pending_event == nil)

env.dfhack_flags = {enable = true, enable_state = false}
run_plugin()
assert(not env.isEnabled() and not eventful.onReport.dfcompanion)
assert(lifecycle.load_enabled_state() == false)
assert(last_status():find('is disabled', 1, true))
local count = #sent_events
env.dfhack_flags = {enable = true, enable_state = false}
run_plugin()
assert(#sent_events == count)
env.dfhack_flags = {module = true}
run_plugin()
assert(not env.isEnabled() and not eventful.onReport.dfcompanion)
fake_dfhack.onStateChange.dfcompanion_lifecycle(env.SC_MAP_UNLOADED)
assert(env.enabled == nil and env.isEnabled() == false)
count = #sent_events
fake_dfhack.onStateChange.dfcompanion_lifecycle(env.SC_MAP_LOADED)
assert(lifecycle.current_session() ~= first_session)
assert(reloaded_lifecycle.current_session() == lifecycle.current_session())
assert(not env.isEnabled() and not eventful.onReport.dfcompanion)
assert(#sent_events == count + 1 and last_status():find('is disabled', 1, true))

env.dfhack_flags = {enable = true, enable_state = true}
run_plugin()
assert(env.isEnabled() and lifecycle.load_enabled_state() == true)
env.pending_event = {text = 'undelivered'}
fake_dfhack.onStateChange.dfcompanion_lifecycle(env.SC_MAP_UNLOADED)
assert(env.enabled == nil and env.pending_event == nil)
current_site_id = 2
fake_dfhack.onStateChange.dfcompanion_lifecycle(env.SC_MAP_LOADED)
assert(not env.isEnabled() and not eventful.onReport.dfcompanion)
assert(lifecycle.load_chat_state(lifecycle.current_session()) == 'zstd:xyz')
assert(not pcall(lifecycle.load_chat_state, session))

fake_dfhack.onStateChange.dfcompanion_lifecycle(env.SC_MAP_UNLOADED)
current_site_id = 3
local error_message
fake_dfhack.printerr = function(message) error_message = message end
fake_dfhack.onStateChange.dfcompanion_lifecycle(env.SC_MAP_LOADED)
assert(env.enabled == nil and not env.isEnabled())
assert(not eventful.onReport.dfcompanion)
assert(error_message and error_message:find('could not restore enabled state', 1, true))
assert(raw_by_site[3] == '{"version":1,"enabled":"false","messages":[]}')

fake_dfhack.onStateChange.dfcompanion_lifecycle(env.SC_MAP_UNLOADED)
current_site_id = 6
service_available = false
fake_dfhack.onStateChange.dfcompanion_lifecycle(env.SC_MAP_LOADED)
assert(env.enabled == nil and raw_by_site[6] == nil)
local facade_path = installed_root
    and installed_root .. '\\hack\\lua\\rpc\\dfcompanion.lua'
    or root .. '\\lua\\rpc\\dfcompanion.lua'
local stub_module = setmetatable({}, {__index = function() return function() end end})
local facade_env
facade_env = {
    mkmodule = function() return setmetatable({}, {__index = facade_env}) end,
    reqscript = function(name) assert(name == 'dfcompanion'); return env end,
    require = function(name)
        if name == 'dfcompanion.internal.lifecycle' then return lifecycle end
        return stub_module
    end,
}
local facade = load_with_env(facade_path, facade_env)
assert(json.decode(facade.lifecycle_state()).site_id == nil)
local facade_session = assert(lifecycle.current_session())
assert(not pcall(facade.food_drink_summary))
assert(not pcall(facade.pause_for_ai_block, 'stale', 'reason'))
assert(pcall(facade.food_drink_summary, facade_session))
site_loaded = false
assert(not pcall(facade.food_drink_summary, facade_session))
site_loaded = true
assert(env.isEnabled() and lifecycle.load_enabled_state() == true)

env.dfhack_flags = {enable = true, enable_state = false}
run_plugin()
assert(not env.isEnabled())
facade.lifecycle_state()
assert(not env.isEnabled() and lifecycle.load_enabled_state() == false)

fake_dfhack.onStateChange.dfcompanion_lifecycle(env.SC_MAP_UNLOADED)
current_site_id = 7
service_available = true
count = #sent_events
fake_dfhack.onStateChange.dfcompanion_lifecycle(env.SC_MAP_LOADED)
assert(env.isEnabled() and lifecycle.load_enabled_state() == true)

fake_dfhack.onStateChange.dfcompanion_lifecycle(env.SC_MAP_UNLOADED)
env.dfhack_flags = {enable = true, enable_state = true}
run_plugin()
fake_dfhack.onStateChange.dfcompanion_lifecycle(env.SC_MAP_UNLOADED)
count = #sent_events
fake_dfhack.onStateChange.dfcompanion_lifecycle(env.SC_MAP_LOADED)
assert(env.isEnabled() and #sent_events == count + 1 and last_status():find('is enabled', 1, true))

print('Enable-state and chat-persistence tests passed.')
