--@ enable = true
--@ module = true

local json = require('json')
local luasocket = require('plugins.luasocket')
local eventful = require('plugins.eventful')
local rpc = require('rpc.dfcompanion')

local COMPANION_HOST = '127.0.0.1'
local COMPANION_PORT = 3000
local RESPONSE_TIMEOUT_SECONDS = 3
local STATE_KEY = 'dfcompanion'
-- Prefix of this script's own announcements, so they are never forwarded.
local MESSAGE_PREFIX = 'DFCompanion:'

enabled = enabled or false

local function http_post(body)
    local client = luasocket.tcp:connect(COMPANION_HOST, COMPANION_PORT)
    local ok, status_code, decoded = pcall(function()
        client:setTimeout(RESPONSE_TIMEOUT_SECONDS, 0)
        client:setBlocking()

        local request = table.concat({
            'POST /api/plugin/events HTTP/1.1',
            'Host: 127.0.0.1:3000',
            'Content-Type: application/json',
            'Content-Length: ' .. #body,
            'Connection: close',
            '',
            body,
        }, '\r\n')
        client:send(request)

        if not client:select(RESPONSE_TIMEOUT_SECONDS, 0) then
            error('Timed out waiting for DFCompanion to begin its HTTP response. ' ..
                'Check that the service is running and listening on 127.0.0.1:3000.')
        end
        local status_line = client:receive('*l')
        if not status_line then
            error('DFCompanion closed the connection without returning an HTTP status line.')
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

        local response_body = ''
        while #response_body < content_length do
            local chunk = client:receive(content_length - #response_body)
            if not chunk or #chunk == 0 then
                error('Connection closed while reading the HTTP response body.')
            end
            response_body = response_body .. chunk
        end

        return response_status, json.decode(response_body)
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
    local ok, status_code, result = pcall(http_post, json.encode(event, {pretty = false}))
    if not ok then
        error('Could not submit an event to DFCompanion: ' .. tostring(status_code), 0)
    end
    if not result or type(result.status) ~= 'string' then
        error('DFCompanion returned an invalid response (HTTP ' .. tostring(status_code) .. ').', 0)
    end
    if result.status ~= 'accepted' then
        error('DFCompanion did not accept an event (HTTP ' .. tostring(status_code) .. '): ' ..
            tostring(result.message), 0)
    end
end

local function disable()
    enabled = false
    dfhack.onStateChange[STATE_KEY] = nil
    eventful.onReport[STATE_KEY] = nil
    print('DFCompanion Lua script disabled.')
end

-- Pauses the game and notifies the player with a one-line alert badge plus a
-- dismissible popup. Unpausing is enough to carry on. Forwarding stays
-- enabled: if the problem persists, the next report pauses the game again.
local function report_failure(err)
    -- Drop the "Could not submit" wrapper, source file prefixes and C++ exception
    -- boilerplate so the popup shows just the cause.
    local detail = tostring(err):gsub('^Could not submit an event to DFCompanion:%s*', '')
        :gsub('[^%s]*%.lua:%d+:%s*', ''):gsub('^.-C%+%+ exception:%s*', '')
    dfhack.world.SetPauseState(true)
    dfhack.gui.showAnnouncement(MESSAGE_PREFIX .. ' problem sending events; game paused.',
        COLOR_LIGHTRED, true)
    -- Popup text is parsed for [tokens]; double brackets to show them literally.
    local popup_detail = detail:gsub('%[', '[['):gsub('%]', ']]')
    dfhack.gui.showPopupAnnouncement(
        'DFCompanion could not send an event, so the game is paused.[B]' ..
        popup_detail .. '[B]Start or fix the DFCompanion service and unpause, ' ..
        'or run "disable dfcompanion" to stop forwarding events.')
    dfhack.printerr(MESSAGE_PREFIX .. ' ' .. tostring(err))
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

    local ok, err = pcall(submit_event, rpc.report_entry(report))
    if not ok then
        report_failure(err)
    end
end

local function enable()
    if not dfhack.world.isFortressMode() or not dfhack.isSiteLoaded() then
        qerror('DFCompanion requires a loaded fortress site.')
    end

    if not enabled then
        submit_event({
            date = rpc.game_date(),
            category = 'test',
            text = 'DFHack Lua connection test; DFCompanion is enabled and forwarding announcements.',
        })
        eventful.enableEvent(eventful.eventType.REPORT, 1)
        eventful.onReport[STATE_KEY] = on_report
        enabled = true
        dfhack.onStateChange[STATE_KEY] = function(state)
            if state == SC_MAP_UNLOADED then
                disable()
            end
        end
        print('DFCompanion Lua script enabled.')
    else
        print('DFCompanion Lua script is already enabled.')
    end
end

function isEnabled()
    return enabled
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
    print(('DFCompanion Lua script is %s.'):format(enabled and 'enabled' or 'disabled'))
elseif args[1] == 'disable' then
    disable()
elseif args[1] == 'enable' or #args == 0 then
    enable()
else
    qerror('Usage: dfcompanion | dfcompanion status | enable dfcompanion | disable dfcompanion')
end
