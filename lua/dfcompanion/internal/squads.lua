local common = require('dfcompanion.internal.common')
local U = common.U
local text = common.text
local position_of = common.position_of
local inventory_item_type = common.inventory_item_type
local inventory_item_wear = common.inventory_item_wear
local paged = common.paged
local reply = common.reply

local SQUAD_SLOT_ITEM_TYPES = {
    body = 'ARMOR', head = 'HELM', pants = 'PANTS', gloves = 'GLOVES',
    shoes = 'SHOES', shield = 'SHIELD', weapon = 'WEAPON',
}

local function squad_member_row(position, index)
    local hf = df.historical_figure.find(position.occupant)
    local unit = hf and df.unit.find(hf.unit_id)
    if not unit then
        return {position_number = index, status = 'Occupant not found.'}
    end
    local worn = {}
    local weapons, damaged = {}, 0
    for _, entry in ipairs(unit.inventory) do
        local kind = inventory_item_type(entry.item)
        worn[kind] = (worn[kind] or 0) + 1
        if kind == 'WEAPON' then
            table.insert(weapons, dfhack.items.getDescription(entry.item, 0))
        end
        if inventory_item_wear(entry.item) > 0 then
            damaged = damaged + 1
        end
    end
    local wanted, missing = {}, {}
    for slot, kind in pairs(SQUAD_SLOT_ITEM_TYPES) do
        local count = #position.equipment.uniform[slot]
        if count > 0 then
            wanted[slot] = count
            if (worn[kind] or 0) < count then
                table.insert(missing, slot .. ' (' .. (worn[kind] or 0) .. ' of ' .. count .. ')')
            end
        end
    end
    table.sort(missing)
    return {
        position_number = index,
        unit_id = unit.id,
        name = text(U.getReadableName(unit)),
        position = position_of(unit),
        uniform_specified = next(wanted) ~= nil,
        uniform_slots_wanted = wanted,
        missing_from_uniform = missing,
        carrying_by_item_type = worn,
        weapons = weapons,
        worn_or_damaged_items = damaged,
        has_quiver = position.equipment.quiver >= 0,
        has_backpack = position.equipment.backpack >= 0,
        has_flask = position.equipment.flask >= 0,
    }
end

-- Squads of the fortress with their members, the uniform each member is
-- supposed to wear, and what each is actually carrying.
local function list_squads(name, offset, limit)
    name = name and name ~= '' and name:lower()
    local rows = {}
    for _, squad in ipairs(df.global.world.squads.all or {}) do
        if (squad.entity_id == df.global.plotinfo.group_id) then
            local label = 
                text(dfhack.translation.translateName(squad.name, true)
                ..(squad.alias ~= '' and ' ('..squad.alias..')' or ''))
            if not name or label:lower():find(name, 1, true) then
                local members, vacant = {}, 0
                local uniformed = 0
                for index, position in ipairs(squad.positions) do
                    if position.occupant >= 0 then
                        local row = squad_member_row(position, index + 1)
                        if row.uniform_specified then
                            uniformed = uniformed + 1
                        end
                        table.insert(members, row)
                    else
                        vacant = vacant + 1
                    end
                end
                local routine = df.global.plotinfo.alerts.routines[squad.cur_routine_idx]
                table.insert(rows, {
                    id = squad.id,
                    name = label,
                    routine = routine and routine.name or nil,
                    member_count = #members,
                    vacant_positions = vacant,
                    members_with_uniform_specified = uniformed,
                    standing_orders = #squad.orders,
                    ammunition_specs = #squad.ammo.ammunition,
                    members = members,
                })
            end
        end
    end
    return reply(paged({
        notes = 'missing_from_uniform compares what each member carries with the uniform slots the squad defines (a member without a defined uniform just wears civilian clothing). carrying_by_item_type counts everything in the unit\'s inventory, clothes included; worn_or_damaged_items counts items with any wear.',
    }, 'squads', rows, offset, limit))
end

local M = {}
M.list_squads = list_squads
return M
