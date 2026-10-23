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

-- The current site's name: "Native Name, \"English Name\"", or "" if no site.
function fortress_name()
    local site = dfhack.world.getCurrentSite()
    if not site then
        return ''
    end
    local native = dfhack.translation.translateName(site.name, false)
    local english = dfhack.translation.translateName(site.name, true)
    if native == '' or native == english then
        return english
    end
    return native .. ', "' .. english .. '"'
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

local MAX_WORKSHOP_JOBS_LISTED = 3
local MAX_PLACES_PAGE_SIZE = 50

-- Number of tiles in a zone, honoring irregular shapes.
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

local function is_finished(building)
    return building:getBuildStage() >= building:getMaxBuildStage()
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

-- Shared paging: returns the rows of `list` selected by offset and limit,
-- with the total and the next offset when more rows remain.
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

-- Housing summary with per-floor counts and one paged list: `list` is
-- 'free_beds' (default) or 'unhoused' (citizens without an owned bedroom);
-- `z` (a game-UI z level, or empty) restricts the beds or citizens listed.
function housing(list, z, offset, limit)
    local data = collect_places()
    z = tonumber(z)
    list = list ~= '' and list or 'free_beds'
    if list ~= 'free_beds' and list ~= 'unhoused' then
        return reply({error = "list must be 'free_beds' or 'unhoused'."})
    end
    local rows = {}
    for _, row in ipairs(data[list]) do
        if not z or (row.position and row.position.z == z) then
            table.insert(rows, row)
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
function list_zones(type_filter, offset, limit)
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
function list_locations()
    return reply({locations = collect_places().locations,
        notes = 'A location with zone_count 0 has no zone assigned, so it cannot function yet.'})
end

-- Workshops and furnaces, optionally only those with `name_filter` in their
-- name or only those with queued jobs.
function list_workshops(name_filter, only_with_jobs, offset, limit)
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
function building_counts()
    return reply({buildings = collect_places().buildings,
        notes = 'Workshops and furnaces are in list_workshops; zones in list_zones and list_locations.'})
end

local MAX_STOCKPILE_CONTENT_GROUPS = 5

-- Stockpiles: where they are, what they accept, and how full they are, with
-- the largest groups of items they hold. `category` keeps only stockpiles that
-- accept that category (e.g. food, wood, bars_blocks); `name` matches the name.
function list_stockpiles(category, name, offset, limit)
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
        local kind = df.item_type[entry.item:getType()]
        worn[kind] = (worn[kind] or 0) + 1
        if kind == 'WEAPON' then
            table.insert(weapons, dfhack.items.getDescription(entry.item, 0))
        end
        if entry.item.getWear and entry.item:getWear() > 0 then
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
        name = U.getReadableName(unit),
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
function list_squads(name, offset, limit)
    name = name ~= '' and name:lower() or nil
    local fort = df.historical_entity.find(df.global.plotinfo.group_id)
    local rows = {}
    for _, squad_id in ipairs(fort and fort.squads or {}) do
        local squad = df.squad.find(squad_id)
        if squad then
            local label = squad.alias ~= '' and squad.alias
                or dfhack.translation.translateName(squad.name, true)
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

local STRANGE_MOODS = {[0] = 'Fey', 'Secretive', 'Possessed', 'Macabre', 'Fell'}
local TICKS_PER_DAY = 1200

local function set_flags(flags)
    local names = {}
    for name, on in pairs(flags) do
        if on then
            table.insert(names, tostring(name))
        end
    end
    table.sort(names)
    return names
end

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
function strange_moods()
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

-- Which of DFHack's automation and gameplay helpers (autofarm,
-- autotraining, tailor, autolabor, ...) are switched on, using the same data
-- as DFHack's control panel. Bugfix helpers are left out: they do not change
-- what happens in the fortress in a way worth reporting. Their actions explain
-- much of what happens, e.g. dwarves changing professions when autotraining runs.
function dfhack_automation()
    local ok, result = pcall(function()
        local common = dfhack.reqscript('internal/control-panel/common')
        local registry = dfhack.reqscript('internal/control-panel/registry')
        local enabled_map = common.get_enabled_map()
        local enabled, disabled = {}, {}
        local seen = {}
        for _, data in ipairs(registry.COMMANDS_BY_IDX) do
            if (data.mode == 'enable' or data.mode == 'repeat' or data.mode == 'system_enable')
                and data.group ~= 'bugfix' then
                local name = common.get_first_word(data.command)
                if not seen[name] then
                    seen[name] = true
                    if enabled_map[data.command] or enabled_map[name] then
                        table.insert(enabled, {name = name, group = data.group,
                            description = common.get_description(data)})
                    else
                        table.insert(disabled, name)
                    end
                end
            end
        end
        return {enabled = enabled, enabled_count = #enabled, available_but_off = disabled}
    end)
    if not ok then
        return json.encode({error = 'Could not read the DFHack control panel data: ' .. tostring(result)})
    end
    return reply(result)
end

-- Whether the dfcompanion script (which forwards game events) is enabled.
function script_enabled()
    local output = dfhack.run_command_silent('enable')
    local state = output:match('dfcompanion:%s+(%S+)')
    return tostring(state == 'on')
end

return _ENV
