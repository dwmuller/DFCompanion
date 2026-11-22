local common = require('dfcompanion.internal.common')
local U = common.U
local text = common.text
local name_of = common.name_of
local ui_z = common.ui_z
local position_of = common.position_of
local words = common.words
local unit_name_by_id = common.unit_name_by_id
local is_finished = common.is_finished
local zone_area = common.zone_area
local zone_contains = common.zone_contains
local paged = common.paged
local reply = common.reply

local MAX_WORKSHOP_JOBS_LISTED = 3

local function zone_row(zone, location_names)
    local row = {
        id = zone.id,
        type = words(df.civzone_type[zone.type] or 'unknown'),
        name = zone.name ~= '' and text(zone.name) or nil,
        location = zone.location_id >= 0 and location_names[zone.location_id] or nil,
        area = zone_area(zone),
        position = {x = (zone.x1 + zone.x2) // 2, y = (zone.y1 + zone.y2) // 2, z = ui_z(zone.z)},
    }
    if zone.assigned_unit_id >= 0 then
        row.assigned_unit = {id = zone.assigned_unit_id, name = unit_name_by_id(zone.assigned_unit_id)}
    end
    if #zone.assigned_units > 0 then
        row.assigned_creature_count = #zone.assigned_units
    end
    return row
end

-- Scans the fortress's buildings, zones and locations once and returns them as
-- Lua tables, uncapped. The exported functions below each return one slice.
local function collect_places()
    local population = {}
    for _, unit in ipairs(df.global.world.units.active) do
        if not U.isDead(unit) and (U.isCitizen(unit) or U.isResident(unit)) then
            population[unit.id] = unit
        end
    end
    local population_count = 0
    for _ in pairs(population) do population_count = population_count + 1 end

    local location_names, locations, location_list = {}, {}, {}
    local site = dfhack.world.getCurrentSite()
    for _, ab in ipairs(site and site.buildings or {}) do
        local location = {
            id = ab.id,
            type = (df.abstract_building_type[ab:getType()] or 'unknown'):lower():gsub('_', ' '),
            name = text(dfhack.translation.translateName(ab.name, true)),
            zone_count = 0,
            area = 0,
        }
        location_names[ab.id] = location.name
        locations[ab.id] = location
        table.insert(location_list, location)
    end

    local groups, group_list, workshops = {}, {}, {}
    local zones, zone_types, bedroom_owners, sleeping_zones = {}, {}, {}, {}
    local bedrooms = {zones = 0, assigned = 0}
    local floors = {}
    local function floor_of(z)
        local floor = floors[z]
        if not floor then
            floor = {z = z, beds = 0, free_beds = 0, bedroom_zones = 0}
            floors[z] = floor
        end
        return floor
    end
    for _, b in ipairs(df.global.world.buildings.all) do
        local btype = b:getType()
        if btype == df.building_type.Civzone then
            local zone_type = words(df.civzone_type[b.type] or 'unknown')
            local area = zone_area(b)
            local summary = zone_types[zone_type]
            if not summary then
                summary = {type = zone_type, count = 0, total_area = 0}
                zone_types[zone_type] = summary
            end
            summary.count = summary.count + 1
            summary.total_area = summary.total_area + area
            local location = locations[b.location_id]
            if location then
                location.zone_count = location.zone_count + 1
                location.area = location.area + area
            end
            if b.type == df.civzone_type.Bedroom then
                table.insert(sleeping_zones, b)
                bedrooms.zones = bedrooms.zones + 1
                local floor = floor_of(ui_z(b.z))
                floor.bedroom_zones = floor.bedroom_zones + 1
                if b.assigned_unit_id >= 0 then
                    bedrooms.assigned = bedrooms.assigned + 1
                    bedroom_owners[b.assigned_unit_id] = true
                end
            else
                if b.type == df.civzone_type.Dormitory then
                    table.insert(sleeping_zones, b)
                end
                table.insert(zones, zone_row(b, location_names))
            end
        else
            local is_workshop = btype == df.building_type.Workshop or btype == df.building_type.Furnace
            if is_workshop then
                local row = {
                    id = b.id,
                    name = text(dfhack.buildings.getName(b)),
                    position = {x = (b.x1 + b.x2) // 2, y = (b.y1 + b.y2) // 2, z = ui_z(b.z)},
                    size = (b.x2 - b.x1 + 1) .. 'x' .. (b.y2 - b.y1 + 1),
                    finished = is_finished(b),
                    job_count = #b.jobs,
                }
                if #b.jobs > 0 then
                    row.jobs = {}
                    for i = 0, math.min(#b.jobs, MAX_WORKSHOP_JOBS_LISTED) - 1 do
                        table.insert(row.jobs, text(dfhack.job.getName(b.jobs[i])))
                    end
                end
                table.insert(workshops, row)
            else
                local name = words(df.building_type[btype] or 'unknown')
                local group = groups[name]
                if not group then
                    group = {name = name, built = 0, under_construction = 0}
                    groups[name] = group
                    table.insert(group_list, group)
                end
                if is_finished(b) then
                    group.built = group.built + 1
                else
                    group.under_construction = group.under_construction + 1
                end
            end
        end
    end
    table.sort(group_list, function(a, b) return a.name < b.name end)
    table.sort(workshops, function(a, b)
        if a.name ~= b.name then return a.name < b.name end
        return a.id < b.id
    end)
    table.sort(zones, function(a, b) return a.id < b.id end)
    local zone_type_list = {}
    for _, summary in pairs(zone_types) do table.insert(zone_type_list, summary) end
    table.sort(zone_type_list, function(a, b) return a.type < b.type end)

    -- A bed is spoken for when it lies in a bedroom zone with an owner. Beds
    -- in a dormitory or in an unowned bedroom are free for the taking.
    local beds, free_beds, free_bed_count = 0, {}, 0
    for _, bed in ipairs(df.global.world.buildings.other.BED) do
        if is_finished(bed) then
            beds = beds + 1
            local bed_floor = floor_of(ui_z(bed.z))
            bed_floor.beds = bed_floor.beds + 1
            local setting, zone_id = 'no zone', nil
            local assigned = false
            for _, zone in ipairs(sleeping_zones) do
                if zone_contains(zone, bed.x1, bed.y1, bed.z) then
                    zone_id = zone.id
                    if zone.type == df.civzone_type.Dormitory then
                        setting = 'dormitory'
                    elseif zone.assigned_unit_id >= 0 then
                        assigned = true
                    else
                        setting = 'unassigned bedroom'
                    end
                    break
                end
            end
            if not assigned then
                free_bed_count = free_bed_count + 1
                bed_floor.free_beds = bed_floor.free_beds + 1
                table.insert(free_beds, {
                    id = bed.id,
                    position = {x = bed.x1, y = bed.y1, z = ui_z(bed.z)},
                    setting = setting,
                    zone_id = zone_id,
                })
            end
        end
    end
    table.sort(free_beds, function(a, b) return a.id < b.id end)
    local without_bedroom = {}
    for id, unit in pairs(population) do
        if not bedroom_owners[id] then
            table.insert(without_bedroom, {id = id, name = name_of(unit), position = position_of(unit)})
        end
    end
    table.sort(without_bedroom, function(a, b) return a.id < b.id end)
    local floor_list = {}
    for _, floor in pairs(floors) do table.insert(floor_list, floor) end
    table.sort(floor_list, function(a, b) return a.z > b.z end)

    return {
        population = population_count,
        housing = {
            beds = beds,
            bedroom_zones = bedrooms.zones,
            bedrooms_assigned = bedrooms.assigned,
            unhoused_count = #without_bedroom,
            free_bed_count = free_bed_count,
            by_floor = floor_list,
        },
        unhoused = without_bedroom,
        free_beds = free_beds,
        locations = location_list,
        zone_types = zone_type_list,
        zones = zones,
        workshops = workshops,
        buildings = group_list,
    }
end

-- Housing summary with per-floor counts and one paged list: `list` is
-- 'free_beds' (default) or 'unhoused' (citizens without an owned bedroom);
-- `z` (a game-UI z level, or empty) restricts the beds or citizens listed.
local function housing(list, z, offset, limit)
    local data = collect_places()
    z = tonumber(z)
    list = list ~= '' and list or 'free_beds'
    if list ~= 'free_beds' and list ~= 'unhoused' then
        return reply({error = "list must be 'free_beds' or 'unhoused'."})
    end
    local rows = {}
    for _, row in ipairs(data[list]) do
        if type(row) == 'table' then
            local position = rawget(row, 'position')
            if not z or (type(position) == 'table' and rawget(position, 'z') == z) then
                table.insert(rows, row)
            end
        end
    end
    local result = {
        population = data.population,
        summary = data.housing,
        list = list,
        notes = 'Bedroom zones are not listed in list_zones. Citizens without a bedroom may still sleep in a ' ..
            'dormitory. free_beds are finished beds not in an owned bedroom (in a dormitory, an unassigned ' ..
            'bedroom, or outside any zone). Positions use the game UI z level.',
    }
    return reply(paged(result, list, rows, offset, limit))
end

-- Non-bedroom zones, optionally only those whose type contains `type_filter`.
local function list_zones(type_filter, offset, limit)
    local data = collect_places()
    type_filter = type_filter:lower()
    local rows = {}
    for _, zone in ipairs(data.zones) do
        if type_filter == '' or zone.type:find(type_filter, 1, true) then
            table.insert(rows, zone)
        end
    end
    local result = {
        zone_types = data.zone_types,
        notes = 'zone_types counts every zone including bedrooms; bedroom zones are not listed here (see ' ..
            'get_housing). Area is in tiles; position is the centre with the game UI z level.',
    }
    return reply(paged(result, 'zones', rows, offset, limit))
end

-- Named locations (temples, hospitals, guildhalls...) with zone counts and area.
local function list_locations()
    local locations = {}
    for _, location in ipairs(collect_places().locations) do
        if location.zone_count > 0 then
            table.insert(locations, location)
        end
    end
    return reply({locations = locations,
        notes = 'Only locations with at least one assigned zone are included.'})
end

-- Workshops and furnaces, optionally only those with `name_filter` in their
-- name or only those with queued jobs.
local function list_workshops(name_filter, only_with_jobs, offset, limit)
    local data = collect_places()
    name_filter = name_filter:lower()
    local rows = {}
    for _, workshop in ipairs(data.workshops) do
        if (name_filter == '' or workshop.name:lower():find(name_filter, 1, true))
                and (only_with_jobs ~= 'true' or workshop.job_count > 0) then
            table.insert(rows, workshop)
        end
    end
    return reply(paged({}, 'workshops', rows, offset, limit))
end

-- Counts of the other building kinds, built and under construction.
local function building_counts()
    return reply({buildings = collect_places().buildings,
        notes = 'Workshops and furnaces are in list_workshops; zones in list_zones and list_locations.'})
end

local MAX_GEOMETRY_PLACES = 50

-- Compact per-level counts assembled from the same player-visible place
-- inventory used by the housing, zone, workshop and stockpile tools.
local function geometry_overview()
    local data = collect_places()
    local levels = {}
    local function level_of(z)
        local level = levels[z]
        if not level then
            level = {
                z = z,
                zones = 0,
                zone_tiles = 0,
                workshops = 0,
                stockpiles = 0,
                stockpile_tiles = 0,
            }
            levels[z] = level
        end
        return level
    end
    for _, floor in ipairs(data.housing.by_floor) do
        local level = level_of(floor.z)
        level.beds = floor.beds
        level.free_beds = floor.free_beds
        level.bedroom_zones = floor.bedroom_zones
    end
    for _, zone in ipairs(data.zones) do
        local level = level_of(zone.position.z)
        level.zones = level.zones + 1
        level.zone_tiles = level.zone_tiles + zone.area
    end
    for _, workshop in ipairs(data.workshops) do
        level_of(workshop.position.z).workshops = level_of(workshop.position.z).workshops + 1
    end
    for _, pile in ipairs(df.global.world.buildings.other.STOCKPILE) do
        local level = level_of(ui_z(pile.z))
        level.stockpiles = level.stockpiles + 1
        level.stockpile_tiles = level.stockpile_tiles + zone_area(pile)
    end
    local rows = {}
    for _, level in pairs(levels) do table.insert(rows, level) end
    table.sort(rows, function(a, b) return a.z > b.z end)
    return reply({
        population = data.population,
        levels = rows,
        notes = 'Counts cover zones, workshops, stockpiles and housing places already visible through the companion tools. '
            .. 'Use get_floor_plan on an individual z level for bounded visible tile geometry.',
    })
end

local function place_category(building)
    local kind = building:getType()
    if kind == df.building_type.Civzone then
        return 'zone', words(df.civzone_type[building.type] or 'unknown')
    elseif kind == df.building_type.Stockpile then
        return 'stockpile', 'stockpile'
    elseif kind == df.building_type.Workshop or kind == df.building_type.Furnace then
        return 'workshop', 'workshop'
    end
    return 'building', words(df.building_type[kind] or 'unknown')
end

-- Player-visible buildings and zones that intersect a bounded area on one
-- UI z level. The compact fixed cap protects the AI turn from dense workshop
-- floors while reporting how many additional rows were omitted.
local function geometry_places(min_x, min_y, max_x, max_y, z)
    min_x, min_y, max_x, max_y, z = tonumber(min_x), tonumber(min_y), tonumber(max_x),
        tonumber(max_y), tonumber(z)
    if not min_x or not min_y or not max_x or not max_y or not z then
        return reply({error = 'Geometry bounds and z must be numbers.'})
    end
    local rows = {}
    for _, building in ipairs(df.global.world.buildings.all) do
        if ui_z(building.z) == z and building.x2 >= min_x and building.x1 <= max_x
                and building.y2 >= min_y and building.y1 <= max_y then
            local category, kind = place_category(building)
            table.insert(rows, {
                id = building.id,
                category = category,
                type = kind,
                name = text(dfhack.buildings.getName(building)),
                finished = is_finished(building),
                bounds = {
                    x1 = building.x1, y1 = building.y1,
                    x2 = building.x2, y2 = building.y2, z = z,
                },
            })
        end
    end
    table.sort(rows, function(a, b)
        if a.category ~= b.category then return a.category < b.category end
        if a.type ~= b.type then return a.type < b.type end
        return a.id < b.id
    end)
    local more = math.max(0, #rows - MAX_GEOMETRY_PLACES)
    while #rows > MAX_GEOMETRY_PLACES do table.remove(rows) end
    return reply({places = rows, more_places = more})
end

local MAX_STOCKPILE_CONTENT_GROUPS = 5

-- Stockpiles: where they are, what they accept, and how full they are, with
-- the largest groups of items they hold. `category` keeps only stockpiles that
-- accept that category (e.g. food, wood, bars_blocks); `name` matches the name.
local function list_stockpiles(category, name, offset, limit)
    category = category ~= '' and category:lower() or nil
    name = name ~= '' and name:lower() or nil
    local rows = {}
    local empty_count = 0
    for _, pile in ipairs(df.global.world.buildings.other.STOCKPILE) do
        local accepts = {}
        for key, on in pairs(pile.settings.flags) do
            if on and type(key) == 'string' then
                table.insert(accepts, key)
            end
        end
        table.sort(accepts)
        local pile_name = dfhack.buildings.getName(pile)
        local accepted = not category
        for _, key in ipairs(accepts) do
            if key == category then
                accepted = true
            end
        end
        if accepted and (not name or pile_name:lower():find(name, 1, true)) then
            local tiles = zone_area(pile)
            local occupied, quantity, stacks = {}, 0, 0
            local groups, order = {}, {}
            for _, item in ipairs(dfhack.buildings.getStockpileContents(pile)) do
                stacks = stacks + 1
                local count = item:getStackSize()
                quantity = quantity + count
                occupied[item.pos.x .. ',' .. item.pos.y] = true
                local description = dfhack.items.getDescription(item, 0)
                if not groups[description] then
                    groups[description] = 0
                    table.insert(order, description)
                end
                groups[description] = groups[description] + count
            end
            table.sort(order, function(a, b) return groups[a] > groups[b] end)
            local contents = {}
            for index = 1, math.min(#order, MAX_STOCKPILE_CONTENT_GROUPS) do
                table.insert(contents, {item = order[index], quantity = groups[order[index]]})
            end
            local used = 0
            for _ in pairs(occupied) do used = used + 1 end
            if stacks == 0 then
                empty_count = empty_count + 1
            end
            table.insert(rows, {
                id = pile.id,
                name = pile_name,
                x1 = pile.x1, y1 = pile.y1, x2 = pile.x2, y2 = pile.y2, z = ui_z(pile.z),
                tiles = tiles,
                tiles_in_use = used,
                percent_tiles_in_use = tiles > 0 and math.floor(100 * used / tiles) or 0,
                stacks = stacks,
                total_quantity = quantity,
                accepts = accepts,
                largest_contents = contents,
                more_item_kinds = math.max(0, #order - MAX_STOCKPILE_CONTENT_GROUPS),
            })
        end
    end
    return reply(paged({
        empty_in_selection = empty_count,
        notes = 'tiles_in_use counts tiles holding at least one item; bins, barrels and other containers hold more than one item per tile, so a pile can be full at less than 100%. accepts lists the categories enabled (not every sub-filter).',
    }, 'stockpiles', rows, offset, limit))
end

local M = {}
M.housing = housing
M.list_zones = list_zones
M.list_locations = list_locations
M.list_workshops = list_workshops
M.building_counts = building_counts
M.geometry_overview = geometry_overview
M.geometry_places = geometry_places
M.list_stockpiles = list_stockpiles
return M
