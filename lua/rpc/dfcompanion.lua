local _ENV = mkmodule('rpc.dfcompanion')

local json = require('json')

-- Functions the companion service calls through DFHack's RunLua RPC. DFHack only
-- allows RunLua on modules whose names start with "rpc." (its documented
-- "*-rpc" and "*.rpc" suffix forms are rejected by an inverted check in DFHack).
-- RunLua stringifies each return value, so functions return JSON text. They
-- must be read-only and must not run caller-supplied code.
--
-- The companion is meant to know what the player knows, so this module decides
-- which creatures the player is aware of, grouped like the game's Creatures
-- screen: residents, pets/livestock, other creatures, and dead/missing.

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

-- The game's UI shows z relative to the surface-referenced level, not the raw
-- map index that DFHack and RemoteFortressReader use.
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

-- The amount to add to a raw map z to get the z shown in the game's UI.
function z_offset()
    return tostring(ui_z(0))
end

local function sex_of(unit)
    if U.isFemale(unit) then
        return 'female'
    elseif U.isMale(unit) then
        return 'male'
    end
    return nil
end

local function base_row(unit)
    return {
        id = unit.id,
        name = name_of(unit),
        race = text(U.getRaceReadableName(unit)),
        sex = sex_of(unit),
        profession = text(U.getProfessionName(unit)),
        age = math.floor(U.getAge(unit) or 0),
        position = position_of(unit),
    }
end

local function unit_name_by_id(id)
    local unit = id and id >= 0 and df.unit.find(id)
    return unit and name_of(unit) or nil
end

local function words(name)
    return (name:gsub('(%l)(%u)', '%1 %2'):lower())
end

-- What the unit is doing: its current job, else its social activity
-- (prayer, play, conversation...), else idle.
local function add_status(row, unit)
    local job = unit.job.current_job
    local job_name = job and text(dfhack.job.getName(job)) or nil
    local activity
    local social = U.getMainSocialActivity(unit)
    if social then
        local event = #social.events > 0 and social.events[#social.events - 1]
        local name = event and df.activity_event_type[event:getType()]
            or df.activity_entry_type[social.type]
        activity = name and words(name)
    end
    row.job = job_name
    row.activity = activity
    row.sleeping = (job and job.job_type == df.job_type.Sleep) or nil
    row.idle = (not job and not activity) or nil
    row.status = job_name or activity or 'idle'
end

local function resident_row(unit)
    local row = base_row(unit)
    add_status(row, unit)
    row.mood = STRESS_NAMES[U.getStressCategory(unit)]
    local squad = unit.military.squad_id >= 0 and df.squad.find(unit.military.squad_id)
    row.squad = squad and text(dfhack.translation.translateName(squad.name, true)) or nil
    local positions = {}
    for _, position in ipairs(U.getNoblePositions(unit) or {}) do
        table.insert(positions, text(position.position.name[0]))
    end
    row.noble_positions = positions
    return row
end

local function pet_row(unit)
    local row = base_row(unit)
    row.domesticated = U.isDomesticated(unit)
    row.war_trained = U.isWar(unit)
    row.hunting_trained = U.isHunter(unit)
    row.grazer = U.isGrazer(unit)
    row.owner = unit_name_by_id(unit.relationship_ids[df.unit_relationship_type.PetOwner])
    row.available_for_adoption = U.isAvailableForAdoption(unit)
    local marks = {}
    for mark, test in pairs({
        slaughter = U.isMarkedForSlaughter, gelding = U.isMarkedForGelding,
        training = U.isMarkedForTraining, taming = U.isMarkedForTaming,
    }) do
        if test(unit) then
            table.insert(marks, mark)
        end
    end
    table.sort(marks)
    row.marked_for = marks
    return row
end

local function other_row(unit)
    local row = base_row(unit)
    row.grazer = U.isGrazer(unit)
    if U.isInvader(unit) or U.isDanger(unit) then
        row.disposition = 'hostile'
    elseif U.isMerchant(unit) then
        row.disposition = 'merchant'
    elseif U.isDiplomat(unit) then
        row.disposition = 'diplomat'
    elseif U.isVisitor(unit) then
        row.disposition = 'visitor'
    elseif U.isWildlife(unit) then
        row.disposition = 'wild animal'
    elseif unit.civ_id ~= -1 and unit.civ_id ~= df.global.plotinfo.civ_id then
        -- Heuristic: a foreign-civ unit that is not a guest is treated as hostile.
        row.disposition = 'hostile'
    else
        row.disposition = 'other'
    end
    return row
end

local function dead_row(unit, status)
    local row = base_row(unit)
    row.status = status
    row.grazer = U.isGrazer(unit)
    -- Only deaths recorded as incidents have a date; many animal deaths do not.
    local incident = df.incident.find(unit.counters.death_id)
    if incident and incident.event_year >= 0 then
        row.died = {
            year = incident.event_year,
            month = incident.event_time // 33600 + 1,
            day = incident.event_time % 33600 // 1200 + 1,
        }
    end
    return row
end

-- A tile the player has seen and where the unit is not lurking in ambush.
local function is_seen(unit)
    return U.isVisible(unit) and not unit.flags1.hidden_in_ambush
end

local function belongs_to_fortress(unit)
    return U.isCitizen(unit) or U.isResident(unit) or U.isFortControlled(unit)
        or (U.isOwnCiv(unit) and U.isTame(unit))
end

-- The creatures the player knows of, as JSON:
-- {residents = [...], pets = [...], other = [...], dead = [...]}
function known_creatures()
    local active = {}
    for _, unit in ipairs(df.global.world.units.active) do
        active[unit.id] = true
    end
    local result = {residents = {}, pets = {}, other = {}, dead = {}}
    for _, unit in ipairs(df.global.world.units.all) do
        if U.isDead(unit) then
            if active[unit.id] and (belongs_to_fortress(unit) or is_seen(unit)) then
                table.insert(result.dead, dead_row(unit, 'deceased'))
            end
        elseif not active[unit.id] then
            if U.isOwnCiv(unit) and U.isTame(unit) then
                table.insert(result.dead, dead_row(unit, 'missing'))
            end
        elseif U.isCitizen(unit) or U.isResident(unit) then
            table.insert(result.residents, resident_row(unit))
        elseif belongs_to_fortress(unit) then
            table.insert(result.pets, pet_row(unit))
        elseif is_seen(unit) then
            table.insert(result.other, other_row(unit))
        end
    end
    return json.encode(result, {pretty = false})
end

local function format_date(year, tick_in_year)
    return string.format('%03d-%02d-%02d:%d', year, tick_in_year // 33600 + 1,
        tick_in_year % 33600 // 1200 + 1, tick_in_year % 1200)
end

-- The current game date as "YYYY-MM-DD:T" (T = ticks into the day, 0-1199).
function game_date()
    return format_date(df.global.cur_year, df.global.cur_year_tick)
end

-- Parses "YYYY-MM-DD" or "YYYY-MM-DD:T" into a tick count since year 0.
-- A date without ticks covers the whole day: it maps to the start of the day
-- for a lower bound and to the end of the day for an upper bound.
local function parse_date(s, is_upper)
    local y, m, d, t = tostring(s):match('^(%d+)-(%d+)-(%d+):(%d+)$')
    if not y then
        y, m, d = tostring(s):match('^(%d+)-(%d+)-(%d+)$')
    end
    if not y then
        error('Invalid game date "' .. tostring(s) .. '"; expected YYYY-MM-DD or YYYY-MM-DD:T.')
    end
    y, m, d = tonumber(y), tonumber(m), tonumber(d)
    if m < 1 or m > 12 or d < 1 or d > 28 then
        error('Invalid game date "' .. s .. '": month must be 1-12 and day 1-28.')
    end
    local base = y * 403200 + (m - 1) * 33600 + (d - 1) * 1200
    if t then
        return base + tonumber(t)
    end
    return is_upper and base + 1199 or base
end

-- Coarse announcement categories. The first rule whose pattern matches the
-- announcement_type name wins; anything unmatched is "other".
local CATEGORY_RULES = {
    {'death', {'DEATH', 'SLAIN', 'KILL', 'MISSING'}},
    {'combat', {'AMBUSH', 'SIEGE', 'INVASION', 'ATTACK', 'COMBAT', 'HUNTING', 'STEALS', 'MEGABEAST', 'FORGOTTEN_BEAST', 'TITAN', 'NIGHT_CREATURE', 'DEMON'}},
    {'mood', {'MOOD', 'BERSERK', 'TANTRUM', 'MELANCHOLY', 'STRESS', 'INSANE'}},
    {'arrivals', {'MIGRANT', 'GUEST', 'LIAISON', 'CARAVAN', 'MERCHANT', 'ARRIVAL', 'VISITOR', 'DIPLOMAT'}},
    {'jobs', {'CANCEL', 'SUSPENDED', 'QUOTA', 'MASTERPIECE', 'ARTIFACT', 'JOB'}},
    {'mining', {'STRUCK', 'FEATURE_DISCOVERY', 'DIG_', 'CAVE_IN', 'CAVEIN', 'FLOOD'}},
    {'population', {'BIRTH', 'GROWS_UP', 'CITIZEN', 'PROFESSION', 'RECRUIT', 'POSITION', 'ELECTION', 'GUILD', 'NOBLE', 'MANDATE', 'REACHED_PEAK'}},
    {'environment', {'WEATHER', 'SEASON', 'EVAPORATE', 'MIST'}},
}
local CATEGORY_NAMES = {other = true}
for _, rule in ipairs(CATEGORY_RULES) do
    CATEGORY_NAMES[rule[1]] = true
end

local function category_of(type_name)
    for _, rule in ipairs(CATEGORY_RULES) do
        for _, pattern in ipairs(rule[2]) do
            if type_name:find(pattern, 1, true) then
                return rule[1]
            end
        end
    end
    return 'other'
end

-- A report in the format shared by get_recent_reports and forwarded events:
-- {id, date, type, category, text}.
function report_entry(r)
    local type_name = df.announcement_type[r.type] or tostring(r.type)
    return {
        id = r.id,
        date = format_date(r.year, r.time),
        type = type_name,
        category = category_of(type_name),
        text = text(r.text),
    }
end

-- Announcements, oldest first, as JSON: {total_matching, latest_id, reports =
-- [{id, date, type, category, text}]}. Arguments: limit, from, to, category,
-- since_id, until_id (strings; "" means unbounded/any). Selects announcements
-- within the inclusive from/to range, the category, and the report ids
-- since_id < id <= until_id. Returns the newest `limit` matches, or, when
-- since_id is given, the oldest `limit` matches so that repeated calls using
-- the last returned id as since_id page forward without gaps. latest_id is the
-- id of the newest report of any kind.
function recent_reports(limit, from, to, category, since_id, until_id)
    limit = tonumber(limit)
    if category == '' then
        category = nil
    end
    if category and not CATEGORY_NAMES[category] then
        error('Unknown category "' .. category .. '".')
    end
    local low = (from and from ~= '') and parse_date(from, false) or nil
    local high = (to and to ~= '') and parse_date(to, true) or nil
    since_id = (since_id and since_id ~= '') and tonumber(since_id) or nil
    until_id = (until_id and until_id ~= '') and tonumber(until_id) or nil
    local reports = df.global.world.status.reports
    local picked, total = {}, 0
    local forward = since_id ~= nil
    local first, last, step = #reports - 1, 0, -1
    if forward then
        first, last, step = 0, #reports - 1, 1
    end
    for i = first, last, step do
        local r = reports[i]
        if r.flags.announcement then
            local when = r.year * 403200 + r.time
            if (not low or when >= low) and (not high or when <= high)
                and (not since_id or r.id > since_id) and (not until_id or r.id <= until_id) then
                local entry = report_entry(r)
                if not category or entry.category == category then
                    total = total + 1
                    if #picked < limit then
                        table.insert(picked, entry)
                    end
                end
            end
        end
    end
    local ordered = picked
    if not forward then
        ordered = {}
        for i = #picked, 1, -1 do
            table.insert(ordered, picked[i])
        end
    end
    local latest_id = #reports > 0 and reports[#reports - 1].id or nil
    return json.encode({total_matching = total, latest_id = latest_id, reports = ordered},
        {pretty = false})
end

-- True for items that count as fortress stock: not carried by creatures, not
-- merchants' goods, and not removed or garbage-collected.
local function is_stock(item)
    local flags = item.flags
    return not (flags.trader or flags.garbage_collect or flags.removed
        or dfhack.items.getHolderUnit(item))
end

-- Need fulfillment bands by focus_level, from DFHack's allneeds script.
local FOCUS_BANDS = {
    {300, 'unfettered'}, {200, 'level-headed'}, {100, 'untroubled'},
    {-999, 'not distracted'}, {-9999, 'unfocused'}, {-99999, 'distracted'},
}
local UNMET_FOCUS = -999
local MAX_THOUGHT_GROUPS = 20

local function focus_band(focus_level)
    for _, band in ipairs(FOCUS_BANDS) do
        if focus_level >= band[1] then return band[2] end
    end
    return 'badly distracted'
end

-- A citizen's or resident's mental state as JSON: mood, stress, recent
-- thoughts (emotions grouped by thought type, newest first) and unmet needs.
-- On failure returns {error = ...}.
function unit_mind(unit_id)
    local unit = df.unit.find(tonumber(unit_id))
    if not unit or U.isDead(unit) or not (U.isCitizen(unit) or U.isResident(unit)) then
        return json.encode({error = 'No living resident with ID ' .. tostring(unit_id) ..
            '. Use a unit ID from list_residents.'})
    end
    local soul = unit.status.current_soul
    if not soul then
        return json.encode({error = 'This unit has no personality data.'})
    end
    local personality = soul.personality
    local category = U.getStressCategory(unit)

    local groups, order = {}, {}
    local emotions = personality.emotions
    for i = #emotions - 1, 0, -1 do
        local emotion = emotions[i]
        local thought = df.unit_thought_type[emotion.thought]
        if thought and thought ~= 'None' then
            local feeling = (df.emotion_type[emotion.type] or '?'):lower()
            local subject
            if thought == 'NeedsUnfulfilled' then
                subject = df.need_type[emotion.subthought]
            end
            local key = thought .. '|' .. feeling .. '|' .. tostring(subject)
            local group = groups[key]
            if not group then
                group = {thought = words(thought), emotion = feeling ~= 'anything' and feeling or nil, count = 0,
                    need = subject and words(subject) or nil}
                if emotion.year >= 0 then
                    group.latest = format_date(emotion.year, emotion.year_tick)
                end
                groups[key] = group
                table.insert(order, group)
            end
            group.count = group.count + 1
        end
    end
    local thoughts = {}
    for i = 1, math.min(#order, MAX_THOUGHT_GROUPS) do
        thoughts[i] = order[i]
    end

    local unmet, met = {}, 0
    for _, need in ipairs(personality.needs) do
        if need.focus_level < UNMET_FOCUS then
            local entry = {need = words(df.need_type[need.id] or tostring(need.id)),
                focus_level = need.focus_level, state = focus_band(need.focus_level),
                importance = need.need_level}
            if need.deity_id >= 0 then
                local figure = df.historical_figure.find(need.deity_id)
                entry.deity = figure and text(dfhack.translation.translateName(
                    dfhack.units.getVisibleName(figure), false)) or nil
            end
            table.insert(unmet, entry)
        else
            met = met + 1
        end
    end
    table.sort(unmet, function(a, b) return a.focus_level < b.focus_level end)

    return json.encode({
        id = unit.id,
        name = name_of(unit),
        mood = STRESS_NAMES[category],
        stress = personality.stress,
        long_term_stress = personality.longterm_stress,
        recent_thoughts = thoughts,
        thought_groups_total = #order,
        unmet_needs = unmet,
        met_need_count = met,
        notes = 'Lower stress is better (negative is calm). Thoughts are grouped by kind with a count and the date of the newest one. Needs are unmet when focus_level is below ' .. UNMET_FOCUS .. '.',
    }, {pretty = false})
end

-- Item groups matching `search` (substring of the item description or type
-- name) and `item_type` (e.g. "WOOD", "DRINK"; "" = any), as JSON. Items are
-- grouped by type and description; groups are sorted by total quantity.
-- Items carried by units and merchants' goods are left out. Arguments are
-- strings; offset and limit are numbers in string form.
function search_items(search, item_type, offset, limit)
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
function food_drink_summary()
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

return _ENV
