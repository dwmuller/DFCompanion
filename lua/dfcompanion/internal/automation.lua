local common = require('dfcompanion.internal.common')
local json = common.json
local reply = common.reply

local function automation_status()
    local control_panel = dfhack.reqscript('internal/control-panel/common')
    local registry = dfhack.reqscript('internal/control-panel/registry')
    local enabled_map = control_panel.get_enabled_map()
    local enabled, disabled = {}, {}
    local seen = {}
    for _, data in ipairs(registry.COMMANDS_BY_IDX) do
        if (data.mode == 'enable' or data.mode == 'repeat' or data.mode == 'system_enable')
            and data.group ~= 'bugfix' then
            local name = control_panel.get_first_word(data.command)
            if not seen[name] then
                seen[name] = true
                if enabled_map[data.command] or enabled_map[name] then
                    table.insert(enabled, {name = name, group = data.group,
                        description = control_panel.get_description(data)})
                else
                    table.insert(disabled, name)
                end
            end
        end
    end
    return {enabled = enabled, enabled_count = #enabled, available_but_off = disabled}
end

local function active_automation()
    local ok, status = pcall(automation_status)
    if not ok then
        return {}, 'Could not read the DFHack control panel data: ' .. tostring(status)
    end
    local names = {}
    for _, entry in ipairs(status.enabled) do
        table.insert(names, entry.name)
    end
    return names
end

-- Which of DFHack's automation and gameplay helpers (autofarm,
-- autotraining, tailor, autolabor, ...) are switched on, using the same data
-- as DFHack's control panel. Bugfix helpers are left out: they do not change
-- what happens in the fortress in a way worth reporting. Their actions explain
-- much of what happens, e.g. dwarves changing professions when autotraining runs.
local function dfhack_automation()
    local ok, result = pcall(automation_status)
    if not ok then
        return json.encode({error = 'Could not read the DFHack control panel data: ' .. tostring(result)})
    end
    return reply(result)
end

local M = {}
M.dfhack_automation = dfhack_automation
M.active_automation = active_automation
return M
