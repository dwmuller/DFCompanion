local common = require('dfcompanion.internal.common')
local creatures = require('dfcompanion.internal.creatures')
local text = common.text
local labor_name = common.labor_name
local U = common.U
local set_flags = common.set_flags
local paged = common.paged
local reply = common.reply
local work_details_vector = creatures.work_details_vector
local work_detail_summary = creatures.work_detail_summary
local work_detail_row = creatures.work_detail_row
local known_residents_by_id = creatures.known_residents_by_id
local resident_row = creatures.resident_row
local MAX_WORK_ORDER_CONDITIONS = 10
local MAX_WORK_ORDER_CONDITION_REAGENTS = 10
local MAX_WORK_ORDER_REQUIREMENTS = 10
local MAX_REACTION_PAGE_SIZE = 50
local WORK_ORDER_FREQUENCIES = {
    [df.workquota_frequency_type.NONE] = 'unknown',
    [df.workquota_frequency_type.OneTime] = 'one_time',
    [df.workquota_frequency_type.Daily] = 'daily',
    [df.workquota_frequency_type.Monthly] = 'monthly',
    [df.workquota_frequency_type.Seasonally] = 'seasonally',
    [df.workquota_frequency_type.Yearly] = 'yearly',
}
local WORK_ORDER_COMPARISONS = {
    [df.logic_condition_type.NONE] = 'unknown',
    [df.logic_condition_type.AtLeast] = 'at_least',
    [df.logic_condition_type.AtMost] = 'at_most',
    [df.logic_condition_type.GreaterThan] = 'greater_than',
    [df.logic_condition_type.LessThan] = 'less_than',
    [df.logic_condition_type.Exactly] = 'exactly',
    [df.logic_condition_type.Not] = 'not_equal',
}
local WORK_ORDER_DEPENDENCIES = {
    [df.workquota_order_condition_type.Activated] = 'activated',
    [df.workquota_order_condition_type.Completed] = 'completed',
}

local function work_order_material(mat_type, mat_index)
    if mat_type == -1 and mat_index == -1 then
        return nil
    end
    local info = dfhack.matinfo.decode(mat_type, mat_index)
    return info and text(info:getToken()) or nil
end

local function work_order_subtype(item_type, subtype)
    if not item_type or item_type == df.item_type.NONE or subtype == -1 then
        return nil
    end
    local def = dfhack.items.getSubtypeDef(item_type, subtype)
    return def and text(def.id) or nil
end

-- Shared factual serialization for reaction reagents, manager-order
-- requirements, and queued-job item requirements. The objects all expose the
-- same DF item-filter fields, but callers supply the applicable quantity and
-- can pass the containing reaction when serializing a raw reagent directly.
-- reagent_index is explicit because manager conditions do not expose that
-- field; reading an absent DFHack struct field raises an error.
local function item_requirement_row(item, quantity, containing_reaction, reagent_index)
    local row = {
        quantity = quantity or item.quantity,
        flags = {},
    }
    if item.item_type ~= df.item_type.NONE then
        row.item_type = df.item_type[item.item_type]
        row.item_subtype = work_order_subtype(item.item_type, item.item_subtype)
    end
    row.material = work_order_material(item.mat_type, item.mat_index)
    if item.metal_ore ~= -1 then
        local ore = df.global.world.raws.inorganics.all[item.metal_ore]
        row.bearing = ore and text(ore.id) or nil
    end
    if item.reaction_class ~= '' then
        row.reaction_class = text(item.reaction_class)
    end
    if item.has_material_reaction_product ~= '' then
        row.reaction_product = text(item.has_material_reaction_product)
    end
    if item.has_tool_use ~= df.tool_uses.NONE then
        row.tool = df.tool_uses[item.has_tool_use]
    end
    if item.min_dimension ~= -1 then
        row.min_dimension = item.min_dimension
    end
    local reaction = containing_reaction
    if not reaction and item.reaction_id ~= nil and item.reaction_id ~= -1 then
        reaction = df.global.world.raws.reactions.reactions[item.reaction_id]
    end
    if reaction then
        row.reaction = text(reaction.code)
        local reagent
        if reagent_index ~= nil and reagent_index ~= -1 then
            row.reagent_index = reagent_index
            reagent = reaction.reagents[reagent_index]
        elseif containing_reaction then
            reagent = item
        end
        if reagent then
            row.reagent = text(reagent.code)
            local reagent_flags = set_flags(reagent.flags)
            if #reagent_flags > 0 then
                row.reagent_flags = reagent_flags
            end
        end
        if item.contains and #item.contains > 0 then
            row.contains = {}
            row.contains_count = #item.contains
            row.contains_truncated = #item.contains > MAX_WORK_ORDER_CONDITION_REAGENTS
            local count = 0
            for _, reagent_index in ipairs(item.contains) do
                count = count + 1
                if count <= MAX_WORK_ORDER_CONDITION_REAGENTS then
                    reagent = reaction.reagents[reagent_index]
                    table.insert(row.contains, reagent and text(reagent.code) or tostring(reagent_index))
                end
            end
        end
    end
    for _, flags in ipairs({item.flags1, item.flags2, item.flags3}) do
        for _, name in ipairs(set_flags(flags)) do
            table.insert(row.flags, name)
        end
    end
    table.sort(row.flags)
    return row
end

local function item_order_condition(condition)
    local row = item_requirement_row(condition, condition.compare_val)
    row.comparison = WORK_ORDER_COMPARISONS[condition.compare_type] or 'unknown'
    return row
end

local function work_order_requirements(order)
    local result = {}
    local items = order.items and order.items.elements
    if not items then
        return result
    end
    for index, item in ipairs(items) do
        if #result >= MAX_WORK_ORDER_REQUIREMENTS then
            break
        end
        local row = item_requirement_row(item, nil, nil, item.reagent_index)
        row.index = index
        table.insert(result, row)
    end
    return result
end

local function reaction_product_row(product)
    local row = {
        token = text(product.product_token),
        to_container = product.product_to_container,
    }
    if product:getType() == df.reaction_product_type.item then
        row.item_type = df.item_type[product.item_type]
        row.item_subtype = work_order_subtype(product.item_type, product.item_subtype)
        row.material = work_order_material(product.mat_type, product.mat_index)
        row.count = product.count
        row.probability = product.probability
        row.product_dimension = product.product_dimension
        local material_source = {}
        if product.get_material.reagent_code ~= '' then
            material_source.reagent = text(product.get_material.reagent_code)
        end
        if product.get_material.product_code ~= '' then
            material_source.product = text(product.get_material.product_code)
        end
        if next(material_source) then
            row.material_source = material_source
        end
    end
    return row
end

local function reaction_row(reaction, index, available)
    local row = {
        index = index,
        code = text(reaction.code),
        name = text(reaction.name),
        skill = df.job_skill[reaction.skill],
        source = reaction.flags.GENERATED and 'generated' or 'raw',
        available = available,
        flags = set_flags(reaction.flags),
        allowed_buildings = {},
        reagents = {},
        products = {},
    }
    if reaction.source_enid >= 0 then
        row.source_entity_id = reaction.source_enid
    end
    if reaction.source_hfid >= 0 then
        row.source_histfig_id = reaction.source_hfid
    end
    for building_index, building_type in ipairs(reaction.building.type) do
        local building = {type = df.building_type[building_type]}
        local subtype = reaction.building.subtype[building_index]
        if subtype and subtype >= 0 then
            building.subtype = subtype
        end
        local custom = reaction.building.custom[building_index]
        if custom and custom ~= '' and custom ~= -1 then
            building.custom = text(custom)
        end
        table.insert(row.allowed_buildings, building)
    end
    for reagent_index, reagent in ipairs(reaction.reagents) do
        local reagent_row
        if reagent:getType() == df.reaction_reagent_type.item then
            reagent_row = item_requirement_row(
                reagent, reagent.quantity, reaction, reagent_index)
            reagent_row.required_dimension = reagent.quantity
            reagent_row.quantity_unit = 'dimension'
        else
            reagent_row = {type = df.reaction_reagent_type[reagent:getType()]}
        end
        reagent_row.code = text(reagent.code)
        table.insert(row.reagents, reagent_row)
    end
    for _, product in ipairs(reaction.products) do
        table.insert(row.products, reaction_product_row(product))
    end
    return row
end

local function reaction_summary(reaction_code)
    if not reaction_code or reaction_code == '' then
        return nil
    end
    local reaction
    for _, candidate in ipairs(df.global.world.raws.reactions.reactions) do
        if candidate.code == reaction_code then
            reaction = candidate
            break
        end
    end
    if not reaction then
        return nil
    end
    local function describe_item(item_type, material, reaction_class, dimension, count)
        local description = item_type or 'item'
        if material then
            description = description .. ' (' .. material .. ')'
        elseif reaction_class then
            description = description .. ' (reaction class ' .. reaction_class .. ')'
        end
        if dimension and dimension >= 0 then
            description = description .. ', ' .. dimension .. ' dimension'
            if item_type == 'BAR' and dimension == 150 then
                count = count or 1
                description = count .. ' bar' .. (count == 1 and '' or 's') ..
                    ' (' .. (material or 'unspecified material') ..
                    ', 150 dimension each)'
            elseif item_type == 'BOULDER' and reaction_class == 'FLUX'
                    and dimension == 1 then
                description = '1 flux boulder'
            end
        end
        return description
    end
    local ingredients = {}
    for _, reagent in ipairs(reaction.reagents) do
        if reagent:getType() == df.reaction_reagent_type.item then
            table.insert(ingredients, describe_item(
                df.item_type[reagent.item_type],
                work_order_material(reagent.mat_type, reagent.mat_index),
                reagent.reaction_class ~= '' and text(reagent.reaction_class) or nil,
                reagent.quantity))
        else
            table.insert(ingredients, text(reagent.code))
        end
    end
    local products = {}
    for _, product in ipairs(reaction.products) do
        if product:getType() == df.reaction_product_type.item then
            table.insert(products, describe_item(
                df.item_type[product.item_type],
                work_order_material(product.mat_type, product.mat_index),
                nil,
                product.product_dimension,
                product.count))
        else
            table.insert(products, text(product.product_token))
        end
    end
    return table.concat(ingredients, ' + ') .. ' -> ' .. table.concat(products, ' + ')
end

local function list_reactions(code_filter, search_filter, available_only, offset, limit)
    code_filter = text(code_filter):upper()
    search_filter = text(search_filter):lower()
    if (code_filter == '') == (search_filter == '') then
        return reply({error = 'Provide exactly one of code or search.'})
    end
    local entity = df.historical_entity.find(df.global.plotinfo.civ_id)
    if available_only == 'true' and not entity then
        return reply({error = 'The current civilization is unavailable.'})
    end
    local permitted = {}
    if entity then
        for _, reaction_index in ipairs(entity.entity_raw.workshops.permitted_reaction_id) do
            permitted[reaction_index] = true
        end
    end
    local reactions = {}
    for index, reaction in ipairs(df.global.world.raws.reactions.reactions) do
        local available = permitted[index] == true
            or (entity ~= nil and reaction.source_enid == entity.id)
        local matches = not (available_only == 'true' and not available)
        if code_filter ~= '' then
            matches = matches and reaction.code:upper() == code_filter
        else
            local searchable = (reaction.code .. ' ' .. reaction.name):lower()
            for _, reagent in ipairs(reaction.reagents) do
                searchable = searchable .. ' ' .. (reagent.code or '')
                if reagent:getType() == df.reaction_reagent_type.item then
                    searchable = searchable .. ' ' .. (reagent.reaction_class or '')
                        .. ' ' .. (work_order_material(reagent.mat_type, reagent.mat_index) or '')
                end
            end
            for _, product in ipairs(reaction.products) do
                searchable = searchable .. ' ' .. (product.product_token or '')
                if product:getType() == df.reaction_product_type.item then
                    searchable = searchable .. ' ' ..
                        (work_order_material(product.mat_type, product.mat_index) or '')
                end
            end
            matches = matches and searchable:lower():find(search_filter, 1, true) ~= nil
        end
        if matches then
            table.insert(reactions, reaction_row(reaction, index, available))
        end
    end
    table.sort(reactions, function(a, b) return a.code < b.code end)
    return reply(paged({
        available_only = available_only == 'true',
        notes = 'Reaction inputs with required_dimension use DF material dimensions, not item counts; a standard bar is 150 dimension. Available means permitted by the current civilization or generated for it, not currently in stock or enabled at a particular workshop.',
    }, 'reactions', reactions, offset, math.min(tonumber(limit) or 10, MAX_REACTION_PAGE_SIZE)))
end

local function manager_order_row(order)
    local row = {
        id = order.id,
        name = text(dfhack.job.getManagerOrderName(order)),
        amount_left = order.amount_left,
        amount_total = order.amount_total,
        is_active = order.status.active,
        is_validated = order.status.validated,
        frequency = WORK_ORDER_FREQUENCIES[order.frequency] or 'unknown',
        item_condition_count = #order.item_conditions,
        order_condition_count = #order.order_conditions,
        item_conditions = {},
        order_conditions = {},
        item_conditions_truncated = #order.item_conditions > MAX_WORK_ORDER_CONDITIONS,
        order_conditions_truncated = #order.order_conditions > MAX_WORK_ORDER_CONDITIONS,
    }
    local requirement_count = order.items and #order.items.elements or 0
    if requirement_count > 0 then
        row.requirements = work_order_requirements(order)
        row.requirement_count = requirement_count
        row.requirements_truncated = requirement_count > MAX_WORK_ORDER_REQUIREMENTS
    end
    if order.item_type ~= df.item_type.NONE then
        row.item_type = df.item_type[order.item_type]
        row.item_subtype = work_order_subtype(order.item_type, order.item_subtype)
    end
    if order.job_type == df.job_type.PrepareMeal then
        row.meal_ingredients = order.mat_type
    else
        row.material = work_order_material(order.mat_type, order.mat_index)
    end
    if order.reaction_name ~= '' then
        row.reaction = text(order.reaction_name)
    end
    local item_categories = set_flags(order.specflag.encrust_flags)
    if #item_categories > 0 then
        row.item_categories = item_categories
    end
    local material_categories = set_flags(order.material_category)
    if #material_categories > 0 then
        row.material_categories = material_categories
    end
    if order.workshop_id ~= -1 then
        row.workshop_id = order.workshop_id
        local workshop = df.building.find(order.workshop_id)
        if workshop then
            row.workshop_name = text(dfhack.buildings.getName(workshop))
        end
    end
    if order.max_workshops ~= 0 then
        row.max_workshops = order.max_workshops
    end
    local search_parts = {row.name}
    for _, key in ipairs({'item_type', 'item_subtype', 'material', 'reaction', 'workshop_name'}) do
        if row[key] then
            table.insert(search_parts, row[key])
        end
    end
    for _, categories in ipairs({row.item_categories or {}, row.material_categories or {}}) do
        for _, category in ipairs(categories) do
            table.insert(search_parts, category)
        end
    end
    for _, requirement in ipairs(row.requirements or {}) do
        for _, key in ipairs({
            'item_type', 'item_subtype', 'material', 'bearing',
            'reaction_class', 'reaction_product', 'tool', 'reagent',
            'reaction',
        }) do
            if requirement[key] then
                table.insert(search_parts, requirement[key])
            end
        end
        for _, flag in ipairs(requirement.flags) do
            table.insert(search_parts, flag)
        end
    end
    local condition_count = 0
    for _, condition in ipairs(order.item_conditions) do
        condition_count = condition_count + 1
        if condition_count <= MAX_WORK_ORDER_CONDITIONS then
            local condition_row = item_order_condition(condition)
            table.insert(row.item_conditions, condition_row)
            for _, key in ipairs({
                'item_type', 'item_subtype', 'material', 'bearing',
                'reaction_class', 'reaction_product', 'tool', 'reagent',
                'reaction',
            }) do
                if condition_row[key] then
                    table.insert(search_parts, condition_row[key])
                end
            end
            for _, flag in ipairs(condition_row.flags) do
                table.insert(search_parts, flag)
            end
            for _, reagent in ipairs(condition_row.contains or {}) do
                table.insert(search_parts, reagent)
            end
        end
    end
    condition_count = 0
    for _, condition in ipairs(order.order_conditions) do
        condition_count = condition_count + 1
        if condition_count <= MAX_WORK_ORDER_CONDITIONS then
            table.insert(row.order_conditions, {
                order_id = condition.order_id,
                condition = WORK_ORDER_DEPENDENCIES[condition.condition] or 'unknown',
                satisfied = condition.flags.satisfied,
            })
        end
    end
    row.search_text = table.concat(search_parts, ' '):lower()
    return row
end

-- Returns work-detail configuration, not effective labor permissions or
-- current jobs. unit_id is restricted to residents in the known-creature list.
local function list_work_details(search_filter, unit_id, labor_filter, offset, limit)
    local work_details = work_details_vector()
    if not work_details then
        return reply({error = 'Work details are unavailable outside fortress mode.'})
    end
    local known_residents = known_residents_by_id()
    local filter_id
    if unit_id ~= '' then
        filter_id = tonumber(unit_id)
        if not filter_id or filter_id < 0 or filter_id % 1 ~= 0
                or not known_residents[filter_id] then
            return reply({error = 'unit_id must identify a known living resident.'})
        end
    end
    labor_filter = (labor_filter or ''):lower()
        :gsub('[^%w]+', '_'):gsub('^_+', ''):gsub('_+$', '')
    if labor_filter ~= '' then
        local known_labor = false
        for id = math.max(0, df.unit_labor._first_item), df.unit_labor._last_item do
            if labor_name(id) == labor_filter then
                known_labor = true
                break
            end
        end
        if not known_labor then
            return reply({error = 'Unknown labor "' .. labor_filter .. '". Use a normalized DF labor name such as furnace_operate or stone_cutting.'})
        end
    end
    search_filter = (search_filter or ''):lower()
    local rows = {}
    for index, detail in ipairs(work_details) do
        local row, assigned_ids = work_detail_row(detail, index, known_residents)
        local relevant = not filter_id or assigned_ids[filter_id]
            or row.assignment_mode == 'everybody'
        local matches_labor = labor_filter == ''
        for _, labor in ipairs(row.labors) do
            if labor == labor_filter then
                matches_labor = true
                break
            end
        end
        local matches_search = search_filter == '' or row.name:lower():find(search_filter, 1, true)
        if not matches_search then
            for _, labor in ipairs(row.labors) do
                local readable_labor = labor:gsub('_', ' ')
                if labor:find(search_filter, 1, true)
                        or readable_labor:find(search_filter, 1, true) then
                    matches_search = true
                    break
                end
            end
        end
        if relevant and matches_labor and matches_search then
            table.insert(rows, row)
        end
    end
    table.sort(rows, function(a, b)
        local a_name, b_name = a.name:lower(), b.name:lower()
        if a_name ~= b_name then return a_name < b_name end
        return a.index < b.index
    end)
    return reply(paged({
        labor = labor_filter ~= '' and labor_filter or nil,
        notes = "assigned_units lists explicit membership among known residents and repeats each resident's global specialized flag. Residents in everybody-mode details may not appear in assigned_units; use get_unit_details with work for an individual resident. assignment_mode is the stored work-detail policy; effective labor permissions are not inferred. The labor filter is an exact normalized labor name.",
    }, 'work_details', rows, offset, limit))
end

local function find_labor_id(name)
    for id = math.max(0, df.unit_labor._first_item), df.unit_labor._last_item do
        if labor_name(id) == name then
            return id
        end
    end
end

local function normalize_labor_filter(labor)
    return (labor or ''):lower():gsub('[^%w]+', '_')
        :gsub('^_+', ''):gsub('_+$', '')
end

-- Bounded roster of known residents with one exact native labor enabled.
local function labor_coverage(labor_filter, idle_only, offset, limit)
    local labor = normalize_labor_filter(labor_filter)
    local labor_id = find_labor_id(labor)
    if not labor_id then
        return reply({error = 'Unknown labor "' .. labor ..
            '". Use a normalized DF labor name such as furnace_operate or stone_cutting.'})
    end
    local work_details = work_details_vector()
    if not work_details then
        return reply({error = 'Work details are unavailable outside fortress mode.'})
    end
    idle_only = idle_only == 'true'

    local covering, everybody_count = {}, 0
    for index, detail in ipairs(work_details) do
        if detail.allowed_labors[labor_id] then
            local row = work_detail_summary(detail, index)
            if row.assignment_mode == 'everybody' then
                everybody_count = everybody_count + 1
            end
            table.insert(covering, row)
        end
    end
    table.sort(covering, function(a, b) return a.index < b.index end)

    local rows, specialized_count, explicit_member_count, idle_count = {}, 0, 0, 0
    for _, unit in ipairs(df.global.world.units.active) do
        if not U.isDead(unit) and (U.isCitizen(unit) or U.isResident(unit))
            and unit.status.labors[labor_id] then
            local resident = resident_row(unit)
            if not idle_only or resident.idle then
                local applicable_details, explicit_member = {}, false
                for _, summary in ipairs(covering) do
                    local detail = work_details[summary.index]
                    local member = false
                    for _, id in ipairs(detail.assigned_units) do
                        if id == unit.id then
                            member = true
                            break
                        end
                    end
                    local applies = summary.assignment_mode == 'everybody'
                        or (summary.assignment_mode ~= 'nobody' and member)
                    if applies then
                        table.insert(applicable_details, summary.name)
                        explicit_member = explicit_member or member
                    end
                end
                local specialized = creatures.is_specialized(unit)
                specialized_count = specialized_count + (specialized and 1 or 0)
                explicit_member_count = explicit_member_count + (explicit_member and 1 or 0)
                idle_count = idle_count + (resident.idle and 1 or 0)
                table.insert(rows, {
                    id = resident.id,
                    name = resident.name,
                    profession = resident.profession,
                    status = resident.status,
                    idle = resident.idle or false,
                    position = resident.position,
                    specialized = specialized,
                    explicit_detail_member = explicit_member,
                    applicable_details = applicable_details,
                })
            end
        end
    end
    table.sort(rows, function(a, b)
        if a.idle ~= b.idle then return a.idle end
        return a.name < b.name
    end)
    return reply(paged({
        labor = labor,
        covering_details = covering,
        everybody_detail_count = everybody_count,
        enabled_count = #rows,
        specialized_count = specialized_count,
        explicit_member_count = explicit_member_count,
        idle_count = idle_count,
        idle_only = idle_only,
        notes = 'enabled_count counts known residents whose current native labor toggle is enabled. applicable_details explains matching work-detail policies; this is not a prediction of which resident will take a job next.',
    }, 'residents', rows, offset, limit))
end

-- Manager orders are distinct from workshop job queues and labor assignments.
local function list_work_orders(search_filter, offset, limit)
    if not dfhack.isMapLoaded() or not dfhack.world.isFortressMode() then
        return reply({error = 'Work orders are unavailable outside fortress mode.'})
    end
    local world = df.global.world
    local orders = world and world.manager_orders and world.manager_orders.all
    if not orders then
        return reply({error = 'Work orders are unavailable outside fortress mode.'})
    end
    search_filter = (search_filter or ''):lower()
    local rows = {}
    for _, order in ipairs(orders) do
        local row = manager_order_row(order)
        if search_filter == '' or row.search_text:find(search_filter, 1, true) then
            row.search_text = nil
            table.insert(rows, row)
        end
    end
    table.sort(rows, function(a, b)
        return a.id < b.id
    end)
    return reply(paged({
        notes = 'amount_left and amount_total are the stored DF fields. is_active and is_validated are raw manager-order flags, not inferred completed/waiting states. order-condition satisfied is the stored DF flag; item conditions are not evaluated and flags4/flags5 are not decoded. Condition arrays include at most ' ..
            MAX_WORK_ORDER_CONDITIONS .. ' entries each, and reagent lists at most ' ..
            MAX_WORK_ORDER_CONDITION_REAGENTS .. ' entries. Requirements are stored job-item ' ..
            'filters, capped at ' .. MAX_WORK_ORDER_REQUIREMENTS ..
            ' entries; truncation is reported. Native flags and reaction reagent flags are factual restrictions, not evaluated availability.',
    }, 'work_orders', rows, offset, limit))
end

local M = {}
M.list_work_details = list_work_details
M.labor_coverage = labor_coverage
M.list_work_orders = list_work_orders
M.item_requirement_row = item_requirement_row
M.get_reaction = list_reactions
M.reaction_summary = reaction_summary
return M
