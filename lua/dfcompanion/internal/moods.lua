local common = require('dfcompanion.internal.common')
local U = common.U
local text = common.text
local ui_z = common.ui_z
local position_of = common.position_of
local set_flags = common.set_flags
local reply = common.reply

local STRANGE_MOODS = {[0] = 'Fey', 'Secretive', 'Possessed', 'Macabre', 'Fell'}
local TICKS_PER_DAY = 1200

-- A readable description of one demanded item, like showmood's.
local function demand_description(element)
    local parts = {}
    if element.mat_type >= 0 then
        local info = dfhack.matinfo.decode(element.mat_type, element.mat_index)
        local name = info and info:toString() or 'material'
        parts[#parts + 1] = name == 'rock' and 'stone' or name
    end
    for _, flags in ipairs({element.flags1, element.flags2, element.flags3}) do
        for _, name in ipairs(set_flags(flags)) do
            if name ~= 'body_part' then
                parts[#parts + 1] = name
            end
        end
    end
    local kind = df.item_type[element.item_type] or 'item'
    parts[#parts + 1] = kind == 'NONE' and 'item' or kind:lower()
    return table.concat(parts, ' ')
end

-- Citizens in a strange mood (fey, secretive, possessed, macabre, fell): what
-- they demand, what has been delivered, their claimed workshop and the time
-- left before they go insane.
local function strange_moods()
    local rows = {}
    for _, unit in ipairs(df.global.world.units.active) do
        if STRANGE_MOODS[unit.mood] and U.isCitizen(unit) and U.isAlive(unit) then
            local row = {
                unit_id = unit.id,
                name = U.getReadableName(unit),
                mood = STRANGE_MOODS[unit.mood],
                skill = df.job_skill[unit.job.mood_skill],
                ticks_left = unit.job.mood_timeout,
                days_left = math.floor(unit.job.mood_timeout / TICKS_PER_DAY),
                position = position_of(unit),
            }
            local job = unit.job.current_job
            local workshop = job and dfhack.job.getHolder(job)
            if workshop then
                row.workshop = {
                    name = dfhack.buildings.getName(workshop),
                    x = workshop.centerx, y = workshop.centery, z = ui_z(workshop.z),
                }
                local demands = {}
                local all_satisfied = true
                for index, element in ipairs(job.job_items.elements) do
                    local have = 0
                    for _, ref in ipairs(job.items) do
                        if ref.job_item_idx == index then
                            have = have + 1
                        end
                    end
                    if have < element.quantity then
                        all_satisfied = false
                    end
                    table.insert(demands, {
                        what = demand_description(element),
                        needed = element.quantity,
                        have = have,
                    })
                end
                row.demands = demands
                row.all_demands_met = all_satisfied
            else
                row.status = 'No workshop claimed yet.'
            end
            table.insert(rows, row)
        end
    end
    return reply({count = #rows, moods = rows,
        notes = 'ticks_left counts down to insanity (1200 ticks per day). Items are matched by the mood job, not by stockpile contents.'})
end

local M = {}
M.strange_moods = strange_moods
return M
