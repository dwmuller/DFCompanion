local PLUGIN_TOKEN = 'replace-this-with-the-same-32-byte-token-as-the-service'

local json = require('json')
local luasocket = require('plugins.luasocket')

local COMPANION_HOST = '127.0.0.1'
local COMPANION_PORT = 3000
local RESPONSE_TIMEOUT_SECONDS = 3

local function http_post(body)
    local client = luasocket.tcp:connect(COMPANION_HOST, COMPANION_PORT)
    local ok, status_code, decoded = pcall(function()
        client:setTimeout(RESPONSE_TIMEOUT_SECONDS, 0)
        client:setBlocking()

        local request = table.concat({
            'POST /api/plugin/events HTTP/1.1',
            'Host: 127.0.0.1:3000',
            'Authorization: Bearer ' .. PLUGIN_TOKEN,
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

local function submit_test_event()
    if not dfhack.world.isFortressMode() or not dfhack.isSiteLoaded() then
        qerror('DFCompanion test events require a loaded fortress site.')
    end

    local event = {
        category = 'manual_test',
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
        description = 'Manual DFHack Lua HTTP connectivity test.',
    }

    local ok, status_code, result = pcall(http_post, json.encode(event, {pretty = false}))
    if not ok then
        qerror('Could not submit event to DFCompanion: ' .. tostring(status_code))
    end
    if not result or type(result.status) ~= 'string' then
        qerror('DFCompanion returned an invalid response (HTTP ' .. tostring(status_code) .. ').')
    end
    if result.status == 'accepted' then
        print('DFCompanion accepted the test event.')
    elseif result.status == 'queue_full' then
        dfhack.world.SetPauseState(true)
        qerror(result.message)
    else
        qerror('DFCompanion rejected the event (HTTP ' .. tostring(status_code) .. '): ' ..
            tostring(result.message))
    end
end

submit_test_event()
