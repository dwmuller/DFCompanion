local common = require('dfcompanion.internal.common')
local json = common.json
local text = common.text
local format_date = common.format_date

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
local function report_entry(r)
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
local function recent_reports(limit, from, to, category, since_id, until_id)
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

local M = {}
M.report_entry = report_entry
M.recent_reports = recent_reports
return M
