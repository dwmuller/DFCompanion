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
        if not (flags.trader or flags.garbage_collect or flags.removed
                or dfhack.items.getHolderUnit(item)) then
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

return _ENV
