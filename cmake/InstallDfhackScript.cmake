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

file(COPY_FILE "${SCRIPT_SOURCE}" "${script_directory}/dfcompanion.lua" ONLY_IF_DIFFERENT)
message(STATUS "Installed dfcompanion.lua to ${script_directory}")
