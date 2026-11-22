local common = require('dfcompanion.internal.common')
local json = common.json
local U = common.U
local STRESS_NAMES = common.STRESS_NAMES
local text = common.text
local name_of = common.name_of
local ui_z = common.ui_z
local position_of = common.position_of
local sex_of = common.sex_of
local unit_name_by_id = common.unit_name_by_id
local words = common.words
local snake = common.snake
local enum_key = common.enum_key
local format_date = common.format_date
local inventory_item_type = common.inventory_item_type
local inventory_item_wear = common.inventory_item_wear
local labor_name = common.labor_name
local automation = require('dfcompanion.internal.automation')

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

-- DFHack's skill classes, as snake_case names: normal (labor skills), medical,
-- personal, social, cultural, military_weapon, military_unarmed,
-- military_attack, military_defense, military_misc.
local function snake(name)
    return (name:gsub('(%l)(%u)', '%1_%2'):lower())
end

local function skills_of(unit)
    local skills = {}
    local soul = unit.status.current_soul
    if soul then
        for _, skill in ipairs(soul.skills) do
            local attrs = df.job_skill.attrs[skill.id]
            table.insert(skills, {
                name = attrs and attrs.caption and attrs.caption:lower()
                    or words(df.job_skill[skill.id] or tostring(skill.id)),
                class = snake(df.job_skill_class[attrs and attrs.type or 0] or 'Normal'),
                level = skill.rating,
                experience = skill.experience,
            })
        end
    end
    table.sort(skills, function(a, b)
        if a.level ~= b.level then return a.level > b.level end
        return a.name < b.name
    end)
    return skills
end

-- Effective attribute values with the unit's caste median for comparison.
local function attributes_of(unit)
    local raw = df.creature_raw.find(unit.race)
    local caste = raw and raw.caste[unit.caste]
    local function collect(enum, value_of, ranges)
        local result = {}
        for id = enum._first_item, enum._last_item do
            local entry = {value = value_of(unit, id)}
            local range = ranges and ranges[id]
            if range then
                entry.caste_median = range[3]
            end
            result[enum[id]:lower()] = entry
        end
        return result
    end
    local attributes = {
        physical = collect(df.physical_attribute_type, U.getPhysicalAttrValue,
            caste and caste.attributes.phys_att_range),
    }
    if unit.status.current_soul then
        attributes.mental = collect(df.mental_attribute_type, U.getMentalAttrValue,
            caste and caste.attributes.ment_att_range)
    end
    return attributes
end

-- Injury state. impaired_parts are body parts that lost function, muscle or
-- structure, or have a severed nerve (a scar or bruise does not count).
local function health_of(unit)
    local body = unit.body
    local missing, impaired = 0, 0
    local status = body.components.body_part_status
    for i = 0, #status - 1 do
        local flags = status[i]
        if flags.missing then
            missing = missing + 1
        elseif flags.organ_loss or flags.muscle_loss or flags.bone_loss
            or flags.motor_nerve_severed or flags.sensory_nerve_severed then
            impaired = impaired + 1
        end
    end
    local infected = 0
    for _, wound in ipairs(body.wounds) do
        if wound.flags.infection then
            infected = infected + 1
        end
    end
    return {
        blood_percent = body.blood_max > 0 and body.blood_count * 100 // body.blood_max or nil,
        wound_count = #body.wounds,
        infected_wounds = infected,
        missing_parts = missing,
        impaired_parts = impaired,
        pain = unit.counters.pain,
        working_limbs = {
            stand = unit.status2.limbs_stand_count, stand_max = unit.status2.limbs_stand_max,
            grasp = unit.status2.limbs_grasp_count, grasp_max = unit.status2.limbs_grasp_max,
        },
    }
end

local mind_of -- defined below, with the need and thought tables it uses

local WORK_DETAIL_MODES = {
    [df.work_detail_mode.Default] = 'default',
    [df.work_detail_mode.EverybodyDoesThis] = 'everybody',
    [df.work_detail_mode.NobodyDoesThis] = 'nobody',
    [df.work_detail_mode.OnlySelectedDoesThis] = 'selected_workers',
}

local function work_details_vector()
    local plotinfo = df.global.plotinfo
    local labor_info = plotinfo and plotinfo.labor_info
    return labor_info and labor_info.work_details or nil
end

local function work_detail_labors(detail)
    local labors = {}
    for id = math.max(0, df.unit_labor._first_item), df.unit_labor._last_item do
        if detail.allowed_labors[id] then
            table.insert(labors, labor_name(id))
        end
    end
    table.sort(labors)
    return labors
end

local function work_detail_summary(detail, index)
    local flags = detail.flags
    return {
        index = index,
        name = text(detail.name),
        assignment_mode = WORK_DETAIL_MODES[flags.mode] or 'unknown',
        built_in = flags.no_modify,
        cannot_be_everybody = flags.cannot_be_everybody,
        labors = work_detail_labors(detail),
    }
end

local function is_specialized(unit)
    return unit.flags4.only_do_assigned_jobs
end

local function work_detail_membership(detail, unit)
    local explicit_member = false
    for _, assigned_id in ipairs(detail.assigned_units) do
        if assigned_id == unit.id then
            explicit_member = true
            break
        end
    end
    local assignment_mode = WORK_DETAIL_MODES[detail.flags.mode] or 'unknown'
    return explicit_member, assignment_mode == 'everybody'
        or (assignment_mode ~= 'nobody' and explicit_member)
end

local function enabled_labors(unit, work_details)
    local result = {}
    for id = math.max(0, df.unit_labor._first_item), df.unit_labor._last_item do
        if unit.status.labors[id] then
            local name = labor_name(id)
            local details = {}
            for index, detail in ipairs(work_details) do
                if detail.allowed_labors[id] then
                    local explicit_member, applies = work_detail_membership(detail, unit)
                    local row = work_detail_summary(detail, index)
                    row.explicit_member = explicit_member
                    row.applies_to_unit = applies
                    table.insert(details, row)
                end
            end
            table.sort(details, function(a, b) return a.index < b.index end)
            table.insert(result, {name = name, details = details})
        end
    end
    table.sort(result, function(a, b) return a.name < b.name end)
    return result
end

local function unit_work(unit, active_automation, automation_error)
    if U.isDead(unit) or not (U.isCitizen(unit) or U.isResident(unit)) then
        return {applicable = false, work_details = {}}
    end
    local work_details = work_details_vector()
    if not work_details then
        error('Work details are unavailable outside fortress mode.', 0)
    end
    local rows = {}
    for index, detail in ipairs(work_details) do
        local row = work_detail_summary(detail, index)
        local explicit_member, applies = work_detail_membership(detail, unit)
        if applies then
            row.explicit_member = explicit_member
            table.insert(rows, row)
        end
    end
    table.sort(rows, function(a, b)
        local a_name, b_name = a.name:lower(), b.name:lower()
        if a_name ~= b_name then return a_name < b_name end
        return a.index < b.index
    end)
    return {
        applicable = true,
        specialized = is_specialized(unit),
        work_details = rows,
        enabled_labor_count = 0,
        enabled_labors = enabled_labors(unit, work_details),
        active_automation = active_automation,
        automation_error = automation_error,
        notes = 'enabled_labors reflects current native unit toggles. Work-detail detail rows explain which policies cover each labor; exact toggle provenance is not stored by Dwarf Fortress. active_automation is contextual and does not prove which system last changed a toggle.',
    }
end

local MAX_UNIT_INVENTORY_ITEMS = 20

local function enum_key(name)
    return name:gsub('(%l)(%u)', '%1_%2'):lower()
end

local function inventory_item_type(item)
    return df.item_type[item:getType()] or 'NONE'
end

local function inventory_item_wear(item)
    return item.getWear and item:getWear() or 0
end

local function inventory_of(unit)
    local total = #unit.inventory
    local items = {}
    for _, entry in ipairs(unit.inventory) do
        if #items >= MAX_UNIT_INVENTORY_ITEMS then
            break
        end
        local item = entry.item
        local mode = df.inv_item_role_type[entry.mode] or 'UNKNOWN'
        local item_row = {
            id = item.id,
            description = text(dfhack.items.getDescription(item, 0)),
            item_type = enum_key(inventory_item_type(item)),
            mode = enum_key(mode),
        }
        local stack_size = item:getStackSize()
        if stack_size > 1 then
            item_row.stack_size = stack_size
        end
        local wear = inventory_item_wear(item)
        if wear > 0 then
            item_row.wear = wear
        end
        table.insert(items, item_row)
    end
    return {
        total_items = total,
        returned_items = #items,
        truncated = total > #items,
        items = items,
    }
end

-- Adds the detail parts the caller asked for (options.parts is a set of
-- skills, attributes, health, mind, inventory, and work). Skills can be
-- limited to one class and a minimum level.
local function add_details(row, unit, options, active_automation, automation_error)
    local parts = options.parts
    if parts.skills then
        local skills = {}
        for _, skill in ipairs(skills_of(unit)) do
            if (not options.skill_class or skill.class == options.skill_class)
                and skill.level >= (options.min_skill_level or 0) then
                table.insert(skills, skill)
            end
        end
        row.skills = skills
    end
    if parts.attributes then
        row.attributes = attributes_of(unit)
    end
    if parts.health then
        row.health = health_of(unit)
    end
    if parts.mind then
        row.mind = mind_of(unit)
    end
    if parts.inventory then
        row.inventory = inventory_of(unit)
    end
    if parts.work then
        row.work = unit_work(unit, active_automation, automation_error)
        if row.work.applicable then
            row.work.enabled_labor_count = #row.work.enabled_labors
        end
    end
    row.carried_item_count = #unit.inventory
    local rider = unit.relationship_ids[df.unit_relationship_type.RiderMount]
    row.rider_id = rider and rider >= 0 and rider or nil
end

-- The creatures the player knows of, as JSON:
-- {residents = [...], pets = [...], other = [...], dead = [...]}
-- `options` (optional) selects creatures to give details for:
-- {ids = {...}, parts = {skills = true, ...}, skill_class, min_skill_level}.
local function known_creatures_data(options)
    local detail_ids = {}
    for _, id in ipairs(options and options.ids or {}) do
        detail_ids[id] = true
    end
    local active = {}
    for _, unit in ipairs(df.global.world.units.active) do
        active[unit.id] = true
    end
    local result = {residents = {}, pets = {}, other = {}, dead = {}}
    local active_automation, automation_error
    if detail_ids and next(detail_ids) and options.parts and options.parts.work then
        active_automation, automation_error = automation.active_automation()
    end
    for _, unit in ipairs(df.global.world.units.all) do
        local group, row
        if U.isDead(unit) then
            if active[unit.id] and (belongs_to_fortress(unit) or is_seen(unit)) then
                group, row = result.dead, dead_row(unit, 'deceased')
            end
        elseif not active[unit.id] then
            if U.isOwnCiv(unit) and U.isTame(unit) then
                group, row = result.dead, dead_row(unit, 'missing')
            end
        elseif U.isCitizen(unit) or U.isResident(unit) then
            group, row = result.residents, resident_row(unit)
        elseif belongs_to_fortress(unit) then
            group, row = result.pets, pet_row(unit)
        elseif is_seen(unit) then
            group, row = result.other, other_row(unit)
        end
        if row then
            if detail_ids[unit.id] then
                add_details(row, unit, options, active_automation or {}, automation_error)
            end
            table.insert(group, row)
        end
    end
    return result
end

-- `options_json` is "" for the plain lists, else the options described above.
local function known_creatures(options_json)
    local options = options_json and options_json ~= '' and json.decode(options_json) or nil
    return json.encode(known_creatures_data(options), {pretty = false})
end

local function known_residents_by_id()
    local known = {}
    for _, row in ipairs(known_creatures_data(nil).residents) do
        local unit = df.unit.find(row.id)
        if unit then
            known[row.id] = {
                id = row.id,
                name = row.name,
                specialized = is_specialized(unit),
            }
        end
    end
    return known
end

local function work_detail_row(detail, index, known_residents)
    local row = work_detail_summary(detail, index)
    local assigned_ids = {}
    for _, id in ipairs(detail.assigned_units) do
        if known_residents[id] then
            assigned_ids[id] = true
        end
    end
    local ids = {}
    for id in pairs(assigned_ids) do
        table.insert(ids, id)
    end
    table.sort(ids)
    row.assigned_count = #ids
    row.assigned_units = {}
    for _, id in ipairs(ids) do
        table.insert(row.assigned_units, known_residents[id])
    end
    return row, assigned_ids
end

local MAX_CANDIDATES = 25

-- The value of the named attribute (e.g. "strength", "focus"), or nil if the
-- name is not an attribute.
local function attribute_value(unit, name)
    local id = df.physical_attribute_type[name:upper()]
    if id ~= nil then
        return U.getPhysicalAttrValue(unit, id)
    end
    id = df.mental_attribute_type[name:upper()]
    if id ~= nil and unit.status.current_soul then
        return U.getMentalAttrValue(unit, id)
    end
end

local function is_attribute_name(name)
    return df.physical_attribute_type[name:upper()] ~= nil
        or df.mental_attribute_type[name:upper()] ~= nil
end

-- Skills of the unit that satisfy the skill filters, best first. With no skill
-- filter every skill qualifies.
local function matching_skills(unit, options)
    local result = {}
    local wanted = options.skill and options.skill:lower()
    local minimum = options.min_skill_level or 0
    for _, skill in ipairs(skills_of(unit)) do
        if skill.level >= minimum
            and (not options.skill_class or skill.class == options.skill_class)
            and (not wanted or skill.name:find(wanted, 1, true)) then
            table.insert(result, skill)
        end
    end
    return result
end

-- Living citizens and residents that satisfy all the given filters, as JSON:
-- {total_matching, candidates = [...]}. Options (all optional, combined with
-- AND): skill_class, skill (name substring), min_skill_level (with either of
-- those, at least one skill must match; default 1), unassigned_to_squad,
-- adults_only, healthy, min_mood (a mood name), profession (substring),
-- attribute with min_attribute_value, sort_by, limit.
local function find_candidates(options_json)
    local options = json.decode(options_json)
    local filtering_skills = options.skill_class or options.skill
    if filtering_skills and not options.min_skill_level then
        options.min_skill_level = 1
    end
    local sort_by = options.sort_by or (filtering_skills and 'skill') or 'name'
    local min_mood
    if options.min_mood then
        for category = 0, #STRESS_NAMES do
            if STRESS_NAMES[category] == options.min_mood then
                min_mood = category
            end
        end
        if not min_mood then
            return json.encode({error = 'Unknown mood "' .. options.min_mood .. '"; use one of: ' ..
                'miserable, very unhappy, unhappy, fine, content, happy, ecstatic.'})
        end
    end
    local sort_attribute = sort_by ~= 'skill' and sort_by ~= 'name' and sort_by ~= 'mood' and sort_by or nil
    for _, name in ipairs({options.attribute or false, sort_attribute or false}) do
        if name and not is_attribute_name(name) then
            return json.encode({error = 'Unknown attribute "' .. name ..
                '". Use a physical or mental attribute name such as strength, agility, toughness, focus or willpower.'})
        end
    end
    if sort_by == 'skill' and not filtering_skills then
        return json.encode({error = 'sort_by skill needs skill_class or skill.'})
    end
    local profession = options.profession and options.profession:lower()

    local found = {}
    for _, unit in ipairs(df.global.world.units.active) do
        if not U.isDead(unit) and (U.isCitizen(unit) or U.isResident(unit))
            and (not options.adults_only or U.isAdult(unit))
            and (not options.unassigned_to_squad or unit.military.squad_id < 0)
            and (not min_mood or U.getStressCategory(unit) >= min_mood)
            and (not profession or text(U.getProfessionName(unit)):lower():find(profession, 1, true)) then
            local entry = {unit = unit}
            local ok = true
            if filtering_skills then
                entry.skills = matching_skills(unit, options)
                ok = #entry.skills > 0
            end
            if ok and options.attribute then
                entry.attribute = attribute_value(unit, options.attribute)
                ok = entry.attribute ~= nil and entry.attribute >= (options.min_attribute_value or 0)
            end
            if ok and options.healthy then
                local health = health_of(unit)
                local limbs = health.working_limbs
                ok = health.impaired_parts == 0 and limbs.stand == limbs.stand_max
                    and limbs.grasp == limbs.grasp_max
            end
            if ok then
                local key
                if sort_by == 'skill' then
                    key = entry.skills[1].level * 1000000 + entry.skills[1].experience
                elseif sort_by == 'mood' then
                    key = U.getStressCategory(unit)
                elseif sort_attribute then
                    key = attribute_value(unit, sort_attribute) or -1
                end
                entry.key = key
                entry.name = name_of(unit)
                table.insert(found, entry)
            end
        end
    end
    table.sort(found, function(a, b)
        if a.key ~= b.key then
            return (a.key or 0) > (b.key or 0)
        end
        return a.name < b.name
    end)

    local limit = math.min(options.limit or 10, MAX_CANDIDATES)
    local candidates = {}
    for i = 1, math.min(limit, #found) do
        local entry = found[i]
        local row = resident_row(entry.unit)
        local candidate = {
            id = row.id, name = row.name, profession = row.profession, age = row.age,
            mood = row.mood, squad = row.squad, status = row.status,
        }
        if entry.skills then
            candidate.matching_skills = {table.unpack(entry.skills, 1, 3)}
        end
        local name = options.attribute or sort_attribute
        if name then
            candidate.attribute = {name = name:lower(), value = attribute_value(entry.unit, name)}
        end
        local health = health_of(entry.unit)
        if health.impaired_parts > 0 or health.infected_wounds > 0 then
            candidate.impaired_parts = health.impaired_parts
            candidate.infected_wounds = health.infected_wounds
        end
        table.insert(candidates, candidate)
    end
    return json.encode({total_matching = #found, candidates = candidates}, {pretty = false})
end

local function format_date(year, tick_in_year)
    return string.format('%03d-%02d-%02d:%d', year, tick_in_year // 33600 + 1,
        tick_in_year % 33600 // 1200 + 1, tick_in_year % 1200)
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

-- A citizen's or resident's mental state: mood, stress, recent thoughts
-- (emotions grouped by thought type, newest first) and unmet needs. On
-- failure returns {error = ...}.
function mind_of(unit)
    if U.isDead(unit) or not (U.isCitizen(unit) or U.isResident(unit)) then
        return {error = 'Only living citizens and residents have mind data.'}
    end
    local soul = unit.status.current_soul
    if not soul then
        return {error = 'This unit has no personality data.'}
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

    return {
        mood = STRESS_NAMES[category],
        stress = personality.stress,
        long_term_stress = personality.longterm_stress,
        recent_thoughts = thoughts,
        thought_groups_total = #order,
        unmet_needs = unmet,
        met_need_count = met,
        notes = 'Lower stress is better (negative is calm). Thoughts are grouped by kind with a count and the date of the newest one. Needs are unmet when focus_level is below ' .. UNMET_FOCUS .. '.',
           }
end

local M = {}
M.known_creatures = known_creatures
M.find_candidates = find_candidates
M.known_residents_by_id = known_residents_by_id
M.work_details_vector = work_details_vector
M.work_detail_summary = work_detail_summary
M.work_detail_row = work_detail_row
M.is_specialized = is_specialized
M.mind_of = mind_of
M.resident_row = resident_row
return M
