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

local function position_of(unit)
    local x, y, z = dfhack.units.getPosition(unit)
    if not x or x < 0 then
        return nil
    end
    return {x = x, y = y, z = z}
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

local function resident_row(unit)
    local row = base_row(unit)
    local job = unit.job.current_job
    row.job = job and text(dfhack.job.getName(job)) or nil
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

-- The current game date, in the same year/month/day form as death dates.
function game_date()
    local tick = df.global.cur_year_tick
    return json.encode({
        year = df.global.cur_year,
        month = tick // 33600 + 1,
        day = tick % 33600 // 1200 + 1,
    }, {pretty = false})
end

return _ENV
