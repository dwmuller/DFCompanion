if(NOT DEFINED DFHACK_INSTALL_DIR OR DFHACK_INSTALL_DIR STREQUAL "")
    message(FATAL_ERROR
        "DFHACK_INSTALL_DIR is not set. Configure CMake with "
        "-DDFHACK_INSTALL_DIR=<DFHack installation directory>."
    )
endif()

set(script_directory "${DFHACK_INSTALL_DIR}/hack/scripts")
if(NOT IS_DIRECTORY "${script_directory}")
    message(FATAL_ERROR
        "DFHack scripts directory not found: ${script_directory}. "
        "Set DFHACK_INSTALL_DIR to the DFHack installation directory."
    )
endif()

# Copies every .lua file under source_root into destination_root, keeping
# subdirectories and skipping files that are already identical. Relative
# files matching exclude_prefix are skipped.
function(install_lua_tree source_root destination_root exclude_prefix)
    file(GLOB_RECURSE lua_files RELATIVE "${source_root}" "${source_root}/*.lua")
    foreach(lua_file IN LISTS lua_files)
        if(exclude_prefix AND lua_file MATCHES "^${exclude_prefix}/")
            continue()
        endif()
        get_filename_component(subdirectory "${lua_file}" DIRECTORY)
        file(MAKE_DIRECTORY "${destination_root}/${subdirectory}")
        file(COPY_FILE "${source_root}/${lua_file}" "${destination_root}/${lua_file}" ONLY_IF_DIFFERENT)
        message(STATUS "Installed ${lua_file} to ${destination_root}")
    endforeach()
endfunction()

# Scripts are run by name from the DFHack console. The project keeps all Lua
# source under one tree, with console scripts separated in lua/plugin/.
install_lua_tree("${LUA_SOURCE_DIR}/plugin" "${script_directory}" "")
# Internal modules formerly lived under rpc.* and were therefore directly
# callable through RunLua. Remove that obsolete namespace on upgrade.
file(REMOVE_RECURSE "${DFHACK_INSTALL_DIR}/hack/lua/rpc/dfcompanion")
# Modules are loaded with require(), which searches hack/lua. DFHack's RunLua
# RPC can only call modules there whose names start with "rpc.".
install_lua_tree("${LUA_SOURCE_DIR}" "${DFHACK_INSTALL_DIR}/hack/lua" "plugin")
