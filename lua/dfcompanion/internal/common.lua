local json = require('json')

local U = dfhack.units

local STRESS_NAMES = {
    [0] = 'miserable', 'very unhappy', 'unhappy', 'fine', 'content', 'happy', 'ecstatic',
}

local function text(s)
    return s and dfhack.df2utf(s) or ''
end

local function name_of(unit)
    return text(dfhack.translation.translateName(U.getVisibleName(unit)))
end

local function ui_z(z)
    return z + df.global.world.map.region_z - 100
end

local function position_of(unit)
    local x, y, z = dfhack.units.getPosition(unit)
    if not x or x < 0 then
        return nil
    end
    return {x = x, y = y, z = ui_z(z)}
end

local function sex_of(unit)
    if U.isFemale(unit) then
        return 'female'
    elseif U.isMale(unit) then
        return 'male'
    end
    return nil
end

local function z_offset()
    return tostring(ui_z(0))
end

local function words(name)
    return (name:gsub('(%l)(%u)', '%1 %2'):lower())
end

local function snake(name)
    return (name:gsub('(%l)(%u)', '%1_%2'):lower())
end

local function enum_key(name)
    return name:gsub('(%l)(%u)', '%1_%2'):lower()
end

local function unit_name_by_id(id)
    local unit = id and id >= 0 and df.unit.find(id)
    return unit and name_of(unit) or nil
end

local function format_date(year, tick_in_year)
    return string.format('%03d-%02d-%02d:%d', year, tick_in_year // 33600 + 1,
        tick_in_year % 33600 // 1200 + 1, tick_in_year % 1200)
end

local function is_finished(building)
    return building:getBuildStage() >= building:getMaxBuildStage()
end

local function zone_area(zone)
    local room = zone.room
    if room.extents and room.width > 0 and room.height > 0 then
        local count = 0
        for i = 0, room.width * room.height - 1 do
            if room.extents[i] ~= 0 then count = count + 1 end
        end
        return count
    end
    return (zone.x2 - zone.x1 + 1) * (zone.y2 - zone.y1 + 1)
end

local function zone_contains(zone, x, y, z)
    if zone.z ~= z or x < zone.x1 or x > zone.x2 or y < zone.y1 or y > zone.y2 then
        return false
    end
    local room = zone.room
    if room.extents and room.width > 0 and room.height > 0 then
        return room.extents[(y - room.y) * room.width + (x - room.x)] ~= 0
    end
    return true
end

local function set_flags(flags)
    local names = {}
    for name, on in pairs(flags) do
        if on then table.insert(names, tostring(name)) end
    end
    table.sort(names)
    return names
end

local function inventory_item_type(item)
    return df.item_type[item:getType()] or 'NONE'
end

local function inventory_item_wear(item)
    return item.getWear and item:getWear() or 0
end

local MAX_PLACES_PAGE_SIZE = 50

local function paged(result, key, list, offset, limit)
    offset = tonumber(offset) or 0
    limit = math.min(tonumber(limit) or 25, MAX_PLACES_PAGE_SIZE)
    local page = {}
    for i = offset + 1, math.min(offset + limit, #list) do
        table.insert(page, list[i])
    end
    result[key] = page
    result.total_matching = #list
    result.offset = offset
    if offset + #page < #list then
        result.next_offset = offset + #page
    end
    return result
end

local function reply(data)
    return json.encode(data, {pretty = false})
end

return {
    json = json,
    U = U,
    STRESS_NAMES = STRESS_NAMES,
    text = text,
    name_of = name_of,
    ui_z = ui_z,
    position_of = position_of,
    sex_of = sex_of,
    z_offset = z_offset,
    words = words,
    snake = snake,
    enum_key = enum_key,
    unit_name_by_id = unit_name_by_id,
    format_date = format_date,
    is_finished = is_finished,
    zone_area = zone_area,
    zone_contains = zone_contains,
    set_flags = set_flags,
    inventory_item_type = inventory_item_type,
    inventory_item_wear = inventory_item_wear,
    paged = paged,
    reply = reply,
}
