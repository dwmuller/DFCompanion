--@ enable = true
--@ module = true

local json = require('json')
local luasocket = require('plugins.luasocket')
local eventful = require('plugins.eventful')

local COMPANION_HOST = '127.0.0.1'
local COMPANION_PORT = 3000
local RESPONSE_TIMEOUT_SECONDS = 3
local STATE_KEY = 'dfcompanion'

enabled = enabled or false

local function http_post(token, body)
    local client = luasocket.tcp:connect(COMPANION_HOST, COMPANION_PORT)
    local ok, status_code, decoded = pcall(function()
        client:setTimeout(RESPONSE_TIMEOUT_SECONDS, 0)
        client:setBlocking()

        local request = table.concat({
            'POST /api/plugin/events HTTP/1.1',
            'Host: 127.0.0.1:3000',
            'Authorization: Bearer ' .. token,
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

local function submit_event(token, category, description)
    if not dfhack.world.isFortressMode() or not dfhack.isSiteLoaded() then
        qerror('DFCompanion requires a loaded fortress site.')
    end

    local event = {
        category = category,
        real_world_time_utc = {
            seconds = os.time(),
            nanos = 0,
        },
        game_world_time = {
            year = dfhack.world.ReadCurrentYear(),
            month = dfhack.world.ReadCurrentMonth(),
            day = dfhack.world.ReadCurrentDay(),
            tick = dfhack.world.ReadCurrentTick(),
        },
        description = description,
    }

    local ok, status_code, result = pcall(http_post, token, json.encode(event, {pretty = false}))
    if not ok then
        qerror('Could not submit event to DFCompanion: ' .. tostring(status_code))
    end
    if not result or type(result.status) ~= 'string' then
        qerror('DFCompanion returned an invalid response (HTTP ' .. tostring(status_code) .. ').')
    end
    if result.status == 'accepted' then
        return
    elseif result.status == 'queue_full' then
        dfhack.world.SetPauseState(true)
        qerror(result.message)
    else
        qerror('DFCompanion rejected the event (HTTP ' .. tostring(status_code) .. '): ' ..
            tostring(result.message))
    end
end

local function disable()
    enabled = false
    dfhack.onStateChange[STATE_KEY] = nil
    eventful.onReport[STATE_KEY] = nil
    print('DFCompanion Lua script disabled.')
end

local function on_report(report_id)
    local report = df.report.find(report_id)
    if not report or not report.text or report.text == '' then
        return
    end

    local category = report.flags.announcement and 'dfhack_announcement' or 'dfhack_report'
    local description = dfhack.df2utf(report.text)
    if description == '' then
        return
    end

    local ok, err = pcall(submit_event, plugin_token, category, description)
    if not ok then
        disable()
        dfhack.printerr('DFCompanion stopped forwarding reports: ' .. tostring(err))
    end
end

local function enable(token)
    if token then
        if #token ~= 64 or not token:match('^%x+$') then
            qerror('The DFCompanion token must be 64 hexadecimal characters. Copy the current command from the DFCompanion web page.')
        end
        plugin_token = token
    elseif not plugin_token then
        qerror('No DFCompanion token is available. Copy and run the current command from the DFCompanion web page.')
    end

    if not enabled then
        submit_event(plugin_token, 'manual_test',
            'DFHack Lua connection test; DFCompanion is enabled and forwarding reports.')
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
elseif args[1] == 'enable' then
    enable(args[2])
elseif #args == 1 then
    enable(args[1])
else
    qerror('Usage: dfcompanion <token> | dfcompanion status | enable dfcompanion | disable dfcompanion')
end
