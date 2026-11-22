local common = require('dfcompanion.internal.common')
local text = common.text
local ui_z = common.ui_z
local words = common.words
local zone_contains = common.zone_contains
local is_finished = common.is_finished
local reply = common.reply

local MAX_VISIBLE_UNITS = 25
local MAX_VISIBLE_ITEMS = 25

local function building_row(building)
    local kind = building:getType()
    local row = {
        id = building.id,
        type = words(df.building_type[kind] or 'unknown'),
        name = text(dfhack.buildings.getName(building)),
        finished = is_finished(building),
        bounds = {
            x1 = building.x1, y1 = building.y1,
            x2 = building.x2, y2 = building.y2, z = ui_z(building.z),
        },
    }
    if df.building_doorst:is_instance(building) or df.building_hatchst:is_instance(building) then
        local flags = building.door_flags
        row.state = {
            closed = flags.closed == true,
            forbidden = flags.forbidden == true,
            operated_by_mechanisms = flags.operated_by_mechanisms == true,
            taken_by_invaders = flags.taken_by_invaders == true,
            used_by_intruder = flags.used_by_intruder == true,
        }
    elseif df.building_bridgest:is_instance(building) then
        local flags = building.gate_flags
        row.state = {
            raised = flags.raised == true,
            raising = flags.raising == true,
            lowering = flags.lowering == true,
            collapsing = flags.collapsing == true,
            has_support = flags.has_support == true,
        }
        row.direction = words(
            df.building_bridgest.T_direction[building.direction] or 'unknown')
    elseif df.building_floodgatest:is_instance(building)
        or df.building_grate_floorst:is_instance(building)
        or df.building_grate_wallst:is_instance(building)
        or df.building_bars_floorst:is_instance(building)
        or df.building_bars_verticalst:is_instance(building) then
        row.state = {closed = building.gate_flags.closed == true}
    end
    return row
end

local function zone_rows(x, y, raw_z)
    local rows = {}
    for _, zone in ipairs(df.global.world.buildings.all) do
        if zone:getType() == df.building_type.Civzone
            and zone_contains(zone, x, y, raw_z) then
            table.insert(rows, {
                id = zone.id,
                type = words(df.civzone_type[zone.type] or 'unknown'),
                name = zone.name ~= '' and text(zone.name) or nil,
                z = ui_z(zone.z),
            })
        end
    end
    table.sort(rows, function(a, b) return a.id < b.id end)
    return rows
end

local function unit_rows(x, y, raw_z)
    local rows, total = {}, 0
    for _, unit in ipairs(df.global.world.units.active) do
        local ux, uy, uz = dfhack.units.getPosition(unit)
        if ux == x and uy == y and uz == raw_z and dfhack.units.isVisible(unit) then
            total = total + 1
            if #rows < MAX_VISIBLE_UNITS then
                table.insert(rows, {
                    id = unit.id,
                    name = text(dfhack.units.getReadableName(unit)),
                    race = text(dfhack.units.getRaceReadableName(unit)),
                    profession = text(dfhack.units.getProfessionName(unit)),
                })
            end
        end
    end
    table.sort(rows, function(a, b) return a.id < b.id end)
    return rows, total > #rows
end

local function item_rows(x, y, raw_z)
    local rows, total = {}, 0
    for _, item in ipairs(df.global.world.items.other.IN_PLAY) do
        local ix, iy, iz = dfhack.items.getPosition(item)
        if ix == x and iy == y and iz == raw_z
            and not dfhack.items.getHolderUnit(item) then
            total = total + 1
            if #rows < MAX_VISIBLE_ITEMS then
                table.insert(rows, {
                    id = item.id,
                    description = text(dfhack.items.getDescription(item, 0)),
                    item_type = common.enum_key(df.item_type[item:getType()] or 'none'),
                    forbidden = item.flags.forbid,
                    stack_size = item:getStackSize(),
                })
            end
        end
    end
    table.sort(rows, function(a, b) return a.id < b.id end)
    return rows, total > #rows
end

-- Reports everything the player can observe at one map tile. Hidden tiles are
-- fully redacted before looking at terrain or nearby objects.
local function inspect_tile(x, y, z)
    x, y, z = tonumber(x), tonumber(y), tonumber(z)
    if not x or not y or not z or x < 0 or y < 0
        or x % 1 ~= 0 or y % 1 ~= 0 or z % 1 ~= 0 then
        return reply({error = 'x, y, and z must be integers within the map.'})
    end
    local raw_z = z - common.ui_z(0)
    local block = dfhack.maps.getTileBlock(x, y, raw_z)
    if not block then
        return reply({error = 'The requested coordinate is outside the loaded map.'})
    end
    local local_x, local_y = x % 16, y % 16
    local designation = block.designation[local_x][local_y]
    if designation.hidden then
        return reply({
            position = {x = x, y = y, z = z},
            hidden = true,
            notes = 'The tile and its contents are hidden; no terrain, object, item, or creature data is disclosed.',
        })
    end
    local tiletype = dfhack.maps.getTileType(x, y, raw_z)
    local tileattrs = tiletype and df.tiletype.attrs[tiletype]
    local building = dfhack.buildings.findAtTile(x, y, raw_z)
    local units, units_truncated = unit_rows(x, y, raw_z)
    local items, items_truncated = item_rows(x, y, raw_z)
    local rows = {
        position = {x = x, y = y, z = z},
        hidden = false,
        tile = {
            type = tiletype and df.tiletype[tiletype] or nil,
            shape = tileattrs and words(df.tiletype_shape[tileattrs.shape] or 'unknown') or nil,
            material = tileattrs and words(df.tiletype_material[tileattrs.material] or 'unknown') or nil,
            outside = designation.outside == true,
            subterranean = designation.subterranean == true,
            light = designation.light == true,
            water = designation.flow_size > 0 and designation.liquid_type ~= df.tile_liquid.Magma
                and designation.flow_size or nil,
            magma = designation.flow_size > 0 and designation.liquid_type == df.tile_liquid.Magma
                and designation.flow_size or nil,
        },
        buildings = building and {building_row(building)} or {},
        zones = zone_rows(x, y, raw_z),
        creatures = units,
        items = items,
    }
    rows.creatures_truncated = units_truncated
    rows.items_truncated = items_truncated
    return reply(rows)
end

local M = {}
M.inspect_tile = inspect_tile
return M
