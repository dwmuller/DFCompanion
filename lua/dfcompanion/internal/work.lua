local common = require('dfcompanion.internal.common')
local creatures = require('dfcompanion.internal.creatures')
local text = common.text
local set_flags = common.set_flags
local paged = common.paged
local reply = common.reply
local work_details_vector = creatures.work_details_vector
local work_detail_summary = creatures.work_detail_summary
local work_detail_row = creatures.work_detail_row
local known_residents_by_id = creatures.known_residents_by_id
local MAX_WORK_ORDER_CONDITIONS = 10
local MAX_WORK_ORDER_CONDITION_REAGENTS = 10
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

local function item_order_condition(condition)
    local row = {
        comparison = WORK_ORDER_COMPARISONS[condition.compare_type] or 'unknown',
        quantity = condition.compare_val,
        flags = {},
    }
    if condition.item_type ~= df.item_type.NONE then
        row.item_type = df.item_type[condition.item_type]
        row.item_subtype = work_order_subtype(condition.item_type, condition.item_subtype)
    end
    row.material = work_order_material(condition.mat_type, condition.mat_index)
    if condition.metal_ore ~= -1 then
        local ore = df.global.world.raws.inorganics.all[condition.metal_ore]
        row.bearing = ore and text(ore.id) or nil
    end
    if condition.reaction_class ~= '' then
        row.reaction_class = text(condition.reaction_class)
    end
    if condition.has_material_reaction_product ~= '' then
        row.reaction_product = text(condition.has_material_reaction_product)
    end
    if condition.has_tool_use ~= df.tool_uses.NONE then
        row.tool = df.tool_uses[condition.has_tool_use]
    end
    if condition.min_dimension ~= -1 then
        row.min_dimension = condition.min_dimension
    end
    if condition.reaction_id ~= -1 then
        local reaction = df.global.world.raws.reactions.reactions[condition.reaction_id]
        if reaction then
            row.reaction = text(reaction.code)
            row.contains = {}
            row.contains_count = #condition.contains
            row.contains_truncated = #condition.contains > MAX_WORK_ORDER_CONDITION_REAGENTS
            local count = 0
            for _, reagent_index in ipairs(condition.contains) do
                count = count + 1
                if count <= MAX_WORK_ORDER_CONDITION_REAGENTS then
                    local reagent = reaction.reagents[reagent_index]
                    table.insert(row.contains, reagent and text(reagent.code) or tostring(reagent_index))
                end
            end
        end
    end
    for _, flags in ipairs({condition.flags1, condition.flags2, condition.flags3}) do
        for _, name in ipairs(set_flags(flags)) do
            table.insert(row.flags, name)
        end
    end
    table.sort(row.flags)
    return row
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
    local condition_count = 0
    for _, condition in ipairs(order.item_conditions) do
        condition_count = condition_count + 1
        if condition_count <= MAX_WORK_ORDER_CONDITIONS then
            local condition_row = item_order_condition(condition)
            table.insert(row.item_conditions, condition_row)
            for _, key in ipairs({
                'item_type', 'item_subtype', 'material', 'bearing',
                'reaction_class', 'reaction_product', 'tool',
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
local function list_work_details(search_filter, unit_id, offset, limit)
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
    search_filter = (search_filter or ''):lower()
    local rows = {}
    for index, detail in ipairs(work_details) do
        local row, assigned_ids = work_detail_row(detail, index, known_residents)
        local relevant = not filter_id or assigned_ids[filter_id]
            or row.assignment_mode == 'everybody'
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
        if relevant and matches_search then
            table.insert(rows, row)
        end
    end
    table.sort(rows, function(a, b)
        local a_name, b_name = a.name:lower(), b.name:lower()
        if a_name ~= b_name then return a_name < b_name end
        return a.index < b.index
    end)
    return reply(paged({
        notes = "assigned_units lists explicit membership among known residents and repeats each resident's global specialized flag. Residents in everybody-mode details may not appear in assigned_units; use get_unit_details with work for an individual resident. assignment_mode is the stored work-detail policy; effective labor permissions are not inferred.",
    }, 'work_details', rows, offset, limit))
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
            MAX_WORK_ORDER_CONDITION_REAGENTS .. ' entries, with truncation reported.',
    }, 'work_orders', rows, offset, limit))
end

local M = {}
M.list_work_details = list_work_details
M.list_work_orders = list_work_orders
return M
