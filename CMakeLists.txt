cmake_minimum_required(VERSION 3.24)

project(DFCompanion LANGUAGES NONE)

find_program(CARGO_EXECUTABLE NAMES cargo REQUIRED)

set(CARGO_MANIFEST "${CMAKE_CURRENT_SOURCE_DIR}/Cargo.toml")
set(CARGO_TARGET_DIR "${CMAKE_BINARY_DIR}/cargo")
set(CARGO_RELEASE_ARGUMENT "$<$<CONFIG:Release>:--release>")
set(DFHACK_INSTALL_DIR "" CACHE PATH "DFHack installation directory")

add_custom_target(dfcompanion ALL
    COMMAND
        "${CARGO_EXECUTABLE}" build
        --manifest-path "${CARGO_MANIFEST}"
        --target-dir "${CARGO_TARGET_DIR}"
        "${CARGO_RELEASE_ARGUMENT}"
    COMMAND_EXPAND_LISTS
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    COMMENT "Building the Rust companion"
    VERBATIM
)

add_custom_target(install-dfhack-script
    COMMAND
        "${CMAKE_COMMAND}"
        "-DDFHACK_INSTALL_DIR=${DFHACK_INSTALL_DIR}"
        "-DSCRIPT_SOURCE=${CMAKE_CURRENT_SOURCE_DIR}/scripts/dfcompanion.lua"
        -P "${CMAKE_CURRENT_SOURCE_DIR}/cmake/InstallDfhackScript.cmake"
    DEPENDS dfcompanion
    COMMENT "Installing DFCompanion Lua script into DFHack"
    VERBATIM
)

include(CTest)
if(BUILD_TESTING)
    add_test(
        NAME rust-tests
        COMMAND
            "${CARGO_EXECUTABLE}" test
            --manifest-path "${CARGO_MANIFEST}"
            --target-dir "${CARGO_TARGET_DIR}"
            "${CARGO_RELEASE_ARGUMENT}"
    )
endif()
