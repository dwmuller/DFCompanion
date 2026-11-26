--@ enable = true
--@ module = true

--[====[
dfcompanion
===========

Tags: fort | interface

Command: "dfcompanion"

    Connects Dwarf Fortress to the local DFCompanion service.

Forwards announced game reports and lifecycle changes over localhost, while the
service uses a separate RPC module to inspect fortress state. If event
delivery fails, DFCompanion pauses the game and retains the failed event for
retry.

Usage
-----

    enable dfcompanion
    disable dfcompanion
    dfcompanion status
]====]

local json = require('json')
local luasocket = require('plugins.luasocket')
local eventful = require('plugins.eventful')
local rpc = require('rpc.dfcompanion')
local lifecycle = require('dfcompanion.internal.lifecycle')

local COMPANION_HOST = '127.0.0.1'
local COMPANION_PORT = 3000
local RESPONSE_TIMEOUT_SECONDS = 3
local STATE_KEY = 'dfcompanion'
-- Prefix of this script's own announcements, so they are never forwarded.
local MESSAGE_PREFIX = 'DFCompanion:'

pending_event = pending_event or nil
local site_state_loaded = false
local on_companion_contact

local function http_post(path, body)
    local client = luasocket.tcp:connect(COMPANION_HOST, COMPANION_PORT)
    local ok, status_code, decoded = pcall(function()
        -- DFHack's socket timeout uses milliseconds on Windows, seconds elsewhere.
        local timeout = dfhack.getOSType() == 'windows'
            and RESPONSE_TIMEOUT_SECONDS * 1000 or RESPONSE_TIMEOUT_SECONDS
        client:setTimeout(timeout, 0)
        client:setBlocking()

        local request = table.concat({
            'POST ' .. path .. ' HTTP/1.1',
            'Host: 127.0.0.1:3000',
            'Content-Type: application/json',
            'Content-Length: ' .. #body,
            'Connection: close',
            '',
            body,
        }, '\r\n')
        client:send(request)

        local status_line = client:receive('*l')
        if not status_line then
            error('DFCompanion closed the connection or timed out before returning an HTTP status line.')
        end
        status_line = status_line:gsub('\r$', '')
        local response_status = tonumber(status_line:match('^HTTP/%d+%.%d+ (%d+)'))
        if not response_status then
            error('Unrecognized response from DFCompanion: ' .. status_line)
        end

        local content_length = 0
        while true do
            local line = client:receive('*l')
            if not line then
                error('Connection closed while reading the HTTP response headers.')
            end
            line = line:gsub('\r$', '')
            if line == '' then
                break
            end
            local name, value = line:match('^([^:]+):%s*(.*)$')
            if name and name:lower() == 'content-length' then
                content_length = tonumber(value) or 0
            end
        end

        local response_body = {}
        for i = 1, content_length do
            local byte = client:receive(1)
            if not byte then
                error('Connection closed or timed out while reading the HTTP response body.')
            end
            response_body[i] = byte
        end

        return response_status, json.decode(table.concat(response_body))
    end)
    local close_ok, close_error = pcall(function() client:close() end)
    if not ok then
        error(status_code)
    end
    if not close_ok then
        error(close_error)
    end
    return status_code, decoded
end

-- Sends one event; raises an error describing the failure if DFCompanion does
-- not accept it.
local function submit_event(event)
    local ok, status_code, result = pcall(http_post, '/api/plugin/events', json.encode(event, {pretty = false}))
    if not ok then
        error('Could not submit an event to DFCompanion: ' .. tostring(status_code), 0)
    end
    if not result or type(result.status) ~= 'string' then
        error('DFCompanion returned an invalid response (HTTP ' .. tostring(status_code) .. ').', 0)
    end
    on_companion_contact()
    if result.status ~= 'accepted' then
        error('DFCompanion did not accept an event (HTTP ' .. tostring(status_code) .. '): ' ..
            tostring(result.message), 0)
    end
end

-- Reports the game's loading state to DFCompanion. Raises an error on failure;
-- the caller decides whether that is worth surfacing.
local function submit_lifecycle(payload)
    local ok, status_code, result = pcall(http_post, '/api/plugin/lifecycle', payload)
    if not ok then
        error('Could not report game state to DFCompanion: ' .. tostring(status_code), 0)
    end
    if not result or type(result.status) ~= 'string' then
        error('DFCompanion returned an invalid lifecycle response (HTTP ' .. tostring(status_code) .. ').', 0)
    end
    on_companion_contact()
    if result.status ~= 'accepted' then
        error('DFCompanion did not accept game state (HTTP ' .. tostring(status_code) .. '): ' ..
            tostring(result.message), 0)
    end
end

local function status_event(is_enabled)
    return {
        date = rpc.game_date(),
        category = 'test',
        text = is_enabled
            and 'DFHack Lua connection test; DFCompanion is enabled and forwarding announcements.'
            or 'DFCompanion is disabled; game announcements are not being forwarded.',
    }
end

-- Like submit_event for a status message, but only logs a failure: it must not
-- pause the game or retain a pending event.
local function announce_status(is_enabled)
    local ok, err = pcall(submit_event, status_event(is_enabled))
    if not ok then
        dfhack.printerr(MESSAGE_PREFIX .. ' could not announce the plugin state: ' .. tostring(err))
    end
end

local function disable()
    local announce = false
    if dfhack.world.isFortressMode() and dfhack.isSiteLoaded() then
        announce = enabled ~= false
        lifecycle.save_enabled_state(false)
        enabled = false
    else
        enabled = nil
    end
    pending_event = nil
    dfhack.onStateChange[STATE_KEY] = nil
    eventful.onReport[STATE_KEY] = nil
    print('DFCompanion Lua script disabled.')
    if announce then announce_status(false) end
end

-- Pauses the game and notifies the player with a one-line alert badge.
-- Unpausing is enough to retry. Forwarding stays enabled: if the problem
-- persists, the next unpause pauses the game again. Modal popups are avoided
-- because opening one from a DF state/report callback can block game input.
local function report_failure(err, event)
    if event then
        pending_event = event
    end
    -- Drop the "Could not submit" wrapper, source file prefixes and C++ exception
    -- boilerplate so the popup shows just the cause.
    local detail = tostring(err):gsub('^Could not submit an event to DFCompanion:%s*', '')
        :gsub('[^%s]*%.lua:%d+:%s*', ''):gsub('^.-C%+%+ exception:%s*', '')
    dfhack.world.SetPauseState(true)
    dfhack.printerr(MESSAGE_PREFIX .. ' could not send an event; game paused. ' ..
        detail .. ' Start or fix the service and unpause, or run "disable dfcompanion".')
end

-- Retries the report that caused the service-outage pause once the player
-- unpauses. If the service is still unavailable, report_failure pauses again.
local function retry_pending_event()
    if enabled == false or not pending_event then
        return
    end
    local event = pending_event
    local ok, err = pcall(submit_event, event)
    if not ok then
        report_failure(err, event)
        return
    end
    pending_event = nil
    dfhack.printerr(MESSAGE_PREFIX .. ' resumed event forwarding.')
end

local function report_ai_block_pause(reason)
    local detail = tostring(reason):gsub('%[', '[['):gsub('%]', ']]')
    dfhack.printerr(MESSAGE_PREFIX .. ' paused the game because AI processing is blocked. ' ..
        detail .. ' Review the DFCompanion warning, then resume AI processing when ready.')
end

-- Reports the game's loading state to DFCompanion so it can gate fort queries.
-- Registered at load, independent of the enable/disable toggle: the companion
-- needs this state even when event forwarding is off. A failure is logged only;
-- the companion also polls for state, so a missed push is not fatal.
local LIFECYCLE_KEY = 'dfcompanion_lifecycle'

local function push_lifecycle_state()
    local ok, err = pcall(function() submit_lifecycle(lifecycle.lifecycle_state()) end)
    if not ok then
        dfhack.printerr(MESSAGE_PREFIX .. ' could not report game state: ' .. tostring(err))
    end
end

local function on_state_change(state)
    if state == SC_PAUSED then
        local reason = rpc.take_ai_block_pause_reason()
        if reason ~= '' then
            report_ai_block_pause(reason)
        end
    elseif state == SC_UNPAUSED then
        retry_pending_event()
    end
end

local function on_report(report_id)
    if not dfhack.world.isFortressMode() or not dfhack.isSiteLoaded() then
        return
    end
    local report = df.report.find(report_id)
    if not report or not report.flags.announcement or not report.text or report.text == ''
        or report.text:sub(1, #MESSAGE_PREFIX) == MESSAGE_PREFIX then
        return
    end

    local event = rpc.report_entry(report)
    local ok, err = pcall(submit_event, event)
    if not ok then
        if enabled == nil then
            dfhack.printerr(MESSAGE_PREFIX .. ' could not send an event while unconfigured: ' .. tostring(err))
        else
            report_failure(err, event)
        end
    end
end

local function start_forwarding()
    eventful.enableEvent(eventful.eventType.REPORT, 1)
    eventful.onReport[STATE_KEY] = on_report
    dfhack.onStateChange[STATE_KEY] = on_state_change
end

on_companion_contact = function()
    if enabled ~= nil or not site_state_loaded
        or not dfhack.world.isFortressMode() or not dfhack.isSiteLoaded() then
        return
    end
    lifecycle.save_enabled_state(true)
    enabled = true
    announce_status(true)
end

function onCompanionContact()
    on_companion_contact()
end

local function restore_enabled_state()
    if not dfhack.world.isFortressMode() or not dfhack.isSiteLoaded() then
        return
    end
    local ok, result = pcall(function()
        local restored = lifecycle.load_enabled_state()
        if restored ~= nil then lifecycle.save_enabled_state(restored) end
        return restored
    end)
    if not ok then
        dfhack.printerr(MESSAGE_PREFIX .. ' could not restore enabled state: ' .. tostring(result))
        return
    end
    site_state_loaded = true
    enabled = result
    if enabled == false then
        announce_status(false)
    else
        start_forwarding()
        if enabled then announce_status(true) end
    end
end

local function on_lifecycle_change(state)
    if state == SC_MAP_UNLOADED then
        if pending_event then
            dfhack.printerr(MESSAGE_PREFIX .. ' discarded an undelivered event on fortress unload.')
        end
        enabled = nil
        site_state_loaded = false
        pending_event = nil
        dfhack.onStateChange[STATE_KEY] = nil
        eventful.onReport[STATE_KEY] = nil
    elseif state == SC_MAP_LOADED then
        restore_enabled_state()
    end
    if state == SC_WORLD_LOADED or state == SC_WORLD_UNLOADED
        or state == SC_MAP_LOADED or state == SC_MAP_UNLOADED then
        push_lifecycle_state()
    end
end

dfhack.onStateChange[LIFECYCLE_KEY] = on_lifecycle_change

local function enable()
    if not dfhack.world.isFortressMode() or not dfhack.isSiteLoaded() then
        qerror('DFCompanion requires a loaded fortress site.')
    end

    if not enabled then
        lifecycle.save_enabled_state(true)
        enabled = true
        site_state_loaded = true
        start_forwarding()

        local event = status_event(true)
        local ok, err = pcall(submit_event, event)
        if not ok then
            report_failure(err, event)
            return
        end
        print('DFCompanion Lua script enabled.')
    else
        print('DFCompanion Lua script is already enabled.')
    end
end

function isEnabled()
    return enabled == true
end

if dfhack.world.isFortressMode() and dfhack.isSiteLoaded() then
    if enabled == nil then
        restore_enabled_state()
    else
        lifecycle.save_enabled_state(enabled)
        site_state_loaded = true
        if enabled then
            start_forwarding()
        else
            dfhack.onStateChange[STATE_KEY] = nil
            eventful.onReport[STATE_KEY] = nil
        end
    end
else
    enabled = nil
    pending_event = nil
    dfhack.onStateChange[STATE_KEY] = nil
    eventful.onReport[STATE_KEY] = nil
end

if dfhack_flags.module then
    return
end

local args = {...}
if dfhack_flags and dfhack_flags.enable then
    if dfhack_flags.enable_state then
        enable()
    else
        disable()
    end
elseif args[1] == 'status' then
    local state = enabled == nil and 'unconfigured' or (enabled and 'enabled' or 'disabled')
    print(('DFCompanion Lua script is %s.'):format(state))
elseif args[1] == 'disable' then
    disable()
elseif args[1] == 'enable' or #args == 0 then
    enable()
else
    qerror('Usage: dfcompanion | dfcompanion status | enable dfcompanion | disable dfcompanion')
end
