local common = require('dfcompanion.internal.common')
local json = common.json
local U = common.U
local text = common.text
local ui_z = common.ui_z

-- True for items that count as fortress stock: not carried by creatures, not
-- merchants' goods, and not removed or garbage-collected.
local function is_stock(item)
    local flags = item.flags
    return not (flags.trader or flags.garbage_collect or flags.removed
        or dfhack.items.getHolderUnit(item))
end

-- Item groups matching `search` (substring of the item description or type
-- name) and `item_type` (e.g. "WOOD", "DRINK"; "" = any), as JSON. Items are
-- grouped by type and description; groups are sorted by total quantity.
-- Items carried by units and merchants' goods are left out. Arguments are
-- strings; offset and limit are numbers in string form.
local function search_items(search, item_type, offset, limit)
    offset, limit = tonumber(offset), tonumber(limit)
    search = (search or ''):lower()
    item_type = (item_type or ''):upper()
    if item_type ~= '' and df.item_type[item_type] == nil then
        return json.encode({error = 'Unknown item type "' .. item_type .. '"; use a DF item type such as WOOD, DRINK, FOOD, BAR or BOULDER.'})
    end
    local groups, order, type_counts, entries = {}, {}, {}, 0
    for _, item in ipairs(df.global.world.items.other.IN_PLAY) do
        local flags = item.flags
        if is_stock(item) then
            local type_name = df.item_type[item:getType()]
            local name = text(dfhack.items.getDescription(item, 0)):gsub(' %[%d+%]$', '')
            if (item_type == '' or type_name == item_type)
                and (search == '' or name:lower():find(search, 1, true)
                    or type_name:lower():find(search, 1, true)) then
                local key = type_name .. '|' .. name
                local group = groups[key]
                if not group then
                    group = {name = name, item_type = type_name:lower(), quantity = 0,
                        forbidden = 0, rotten = 0, locations = {}}
                    groups[key] = group
                    table.insert(order, group)
                end
                local count = item:getStackSize()
                group.quantity = group.quantity + count
                if flags.forbid then group.forbidden = group.forbidden + count end
                if flags.rotten then group.rotten = group.rotten + count end
                type_counts[type_name:lower()] = (type_counts[type_name:lower()] or 0) + count
                entries = entries + 1
                local x, y, z = dfhack.items.getPosition(item)
                if x and #group.locations < 3 then
                    local loc = {x = x, y = y, z = ui_z(z)}
                    local dup = false
                    for _, seen in ipairs(group.locations) do
                        dup = dup or (seen.x == loc.x and seen.y == loc.y and seen.z == loc.z)
                    end
                    if not dup then table.insert(group.locations, loc) end
                end
            end
        end
    end
    table.sort(order, function(a, b)
        if a.quantity ~= b.quantity then return a.quantity > b.quantity end
        return a.name < b.name
    end)
    local page = {}
    for i = offset + 1, math.min(#order, offset + limit) do
        local group = order[i]
        if group.forbidden == 0 then group.forbidden = nil end
        if group.rotten == 0 then group.rotten = nil end
        table.insert(page, group)
    end
    local result = {total_matching = #order, item_stacks = entries,
        quantity_by_type = type_counts, offset = offset, returned = #page, items = page}
    if offset + #page < #order then
        result.next_offset = offset + #page
    end
    return json.encode(result, {pretty = false})
end

-- Item types counted as food and drink, by summary category. These match what
-- the game's status header counts; plants are reported separately.
local FOOD_CATEGORIES = {
    prepared_meals = {'FOOD'},
    meat = {'MEAT'},
    fish = {'FISH', 'FISH_RAW'},
    dairy_and_eggs = {'CHEESE', 'EGG'},
}
local PLANT_TYPES = {PLANT = true, PLANT_GROWTH = true}
-- Rough consumption: a dwarf gets thirsty after about 25,000 ticks and hungry
-- after about 50,000, and one drink or meal resets the need.
local TICKS_PER_DAY = 1200
local DRINK_INTERVAL_TICKS = 25000
local FOOD_INTERVAL_TICKS = 50000
local MAX_VARIETIES = 15

-- Food and drink stocks as JSON: totals and rough days of supply for drink
-- and for food, plus the most plentiful kinds of each. Quantities are
-- individual units (stack sizes), as the game's status header counts them.
local function food_drink_summary()
    local type_category = {DRINK = 'drink'}
    for category, types in pairs(FOOD_CATEGORIES) do
        for _, type_name in ipairs(types) do
            type_category[type_name] = category
        end
    end
    local drink = {total = 0, available = 0, forbidden = 0, rotten = 0}
    local food = {total = 0, available = 0, forbidden = 0, rotten = 0}
    local by_category, varieties = {}, {drink = {}, food = {}}
    for category in pairs(FOOD_CATEGORIES) do
        by_category[category] = 0
    end
    local edible_raw_plants = 0
    for _, item in ipairs(df.global.world.items.other.IN_PLAY) do
        local type_name = df.item_type[item:getType()]
        local category = type_category[type_name]
        if PLANT_TYPES[type_name] and is_stock(item) and not item.flags.forbid
                and not item.flags.rotten then
            local info = dfhack.matinfo.decode(item)
            if info and info.material.flags.EDIBLE_RAW then
                edible_raw_plants = edible_raw_plants + item:getStackSize()
            end
        end
        if category and is_stock(item) then
            local is_drink = category == 'drink'
            local totals = is_drink and drink or food
            local count = item:getStackSize()
            totals.total = totals.total + count
            if item.flags.forbid then
                totals.forbidden = totals.forbidden + count
            elseif item.flags.rotten then
                totals.rotten = totals.rotten + count
            else
                totals.available = totals.available + count
            end
            if not is_drink then
                by_category[category] = by_category[category] + count
            end
            local name = text(dfhack.items.getDescription(item, 0)):gsub(' %[%d+%]$', '')
            local list = is_drink and varieties.drink or varieties.food
            local entry = list[name]
            if not entry then
                entry = {name = name, quantity = 0}
                if not is_drink then entry.item_type = type_name:lower() end
                list[name] = entry
            end
            entry.quantity = entry.quantity + count
        end
    end

    local population = 0
    for _, unit in ipairs(df.global.world.units.active) do
        if not U.isDead(unit) and (U.isCitizen(unit) or U.isResident(unit)) then
            population = population + 1
        end
    end

    local function top(map)
        local list = {}
        for _, entry in pairs(map) do table.insert(list, entry) end
        table.sort(list, function(a, b)
            if a.quantity ~= b.quantity then return a.quantity > b.quantity end
            return a.name < b.name
        end)
        local total = #list
        while #list > MAX_VARIETIES do table.remove(list) end
        return list, total
    end
    local function days_of_supply(available, interval)
        if population == 0 then return nil end
        return math.floor(available * interval / TICKS_PER_DAY / population * 10 + 0.5) / 10
    end

    drink.days_of_supply = days_of_supply(drink.available, DRINK_INTERVAL_TICKS)
    drink.varieties, drink.variety_count = top(varieties.drink)
    food.days_of_supply = days_of_supply(food.available, FOOD_INTERVAL_TICKS)
    food.by_category = by_category
    food.edible_raw_plants = edible_raw_plants
    food.most_plentiful, food.kind_count = top(varieties.food)
    return json.encode({
        population = population,
        assumptions = 'days_of_supply = available units / (population * consumption). ' ..
            'Assumes each citizen or resident needs one drink per ' ..
            DRINK_INTERVAL_TICKS // TICKS_PER_DAY .. ' days and one meal per ' ..
            FOOD_INTERVAL_TICKS // TICKS_PER_DAY .. ' days; animals are not counted. ' ..
            'Available excludes forbidden and rotten units. Food counts prepared meals, meat, fish, ' ..
            'eggs and dairy, like the game header; edible_raw_plants (fruit, vegetables) is extra ' ..
            'food not included in days_of_supply. It is a rough estimate.',
        drink = drink,
        food = food,
    }, {pretty = false})
end

local M = {}
M.search_items = search_items
M.food_drink_summary = food_drink_summary
return M
