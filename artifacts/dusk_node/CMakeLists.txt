cmake_minimum_required(VERSION 3.20)

if(CMAKE_CURRENT_SOURCE_DIR STREQUAL CMAKE_CURRENT_BINARY_DIR)
    message(FATAL_ERROR
        "In-source builds are not supported. Configure into a separate directory, "
        "e.g. `cmake -S . -B build/<name>` or `cmake --preset <name>`. "
        "This attempt already created CMakeCache.txt and CMakeFiles/ here; delete them.")
endif()

project(dusk_node LANGUAGES C)

find_program(DUSK_NODE_CARGO cargo REQUIRED)

get_filename_component(DUSK_NODE_WORKSPACE "${CMAKE_CURRENT_LIST_DIR}/../.." ABSOLUTE)

set(DUSK_NODE_CARGO_TARGET "" CACHE STRING
    "Rust target triple to build the node for; required. See docs/docs/embedding/node-artifacts.md")
set(DUSK_NODE_CARGO_PROFILE "" CACHE STRING
    "Cargo profile to build the node with; empty follows CMAKE_BUILD_TYPE")
set(DUSK_NODE_IMPL "" CACHE STRING
    "Which impl the node links; required. See docs/docs/embedding/node-artifacts.md")
set(DUSK_NODE_RUSTFLAGS "" CACHE STRING
    "Extra flags for rustc, separated by spaces as in RUSTFLAGS. See docs/docs/embedding/node-artifacts.md")
set(DUSK_NODE_INIT_SCRIPT "" CACHE STRING
    "The shell script the node runs at boot; empty leaves it to cargo: DUSK_NODE_INIT_SCRIPT in the environment, else .cargo/config.toml. See docs/docs/embedding/node-artifacts.md")
set(DUSK_NODE_CARGO_SUBCOMMAND "build" CACHE STRING
    "The cargo subcommand that builds the node. See docs/docs/embedding/node-artifacts.md")

# Per-build-dir cargo output: builds with different impls, triples or profiles
# never overwrite each other's final artifacts, and removing the build dir
# removes everything.
set(DUSK_NODE_CARGO_TARGET_DIR "${CMAKE_CURRENT_BINARY_DIR}/cargo")
set(DUSK_NODE_INCLUDE_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/include")
file(MAKE_DIRECTORY "${DUSK_NODE_INCLUDE_DIRECTORY}")

if(DUSK_NODE_CARGO_PROFILE STREQUAL "")
    if(CMAKE_BUILD_TYPE STREQUAL "Debug")
        set(DUSK_NODE_CARGO_PROFILE dev)
    elseif(CMAKE_BUILD_TYPE STREQUAL "MinSizeRel")
        set(DUSK_NODE_CARGO_PROFILE prod)
    else()
        set(DUSK_NODE_CARGO_PROFILE release)
    endif()
endif()

if(DUSK_NODE_CARGO_PROFILE STREQUAL "dev")
    set(DUSK_NODE_PROFILE_DIRECTORY debug)
else()
    set(DUSK_NODE_PROFILE_DIRECTORY "${DUSK_NODE_CARGO_PROFILE}")
endif()

if(DUSK_NODE_CARGO_TARGET STREQUAL "")
    message(FATAL_ERROR
        "DUSK_NODE_CARGO_TARGET is not set. It names the Rust target triple to build "
        "the node for, and there is no default: CMAKE_SYSTEM_NAME names a platform, not "
        "a triple, so deriving one from it picks the wrong library naming and the wrong "
        "system libraries. Pass the triple you want, the host's included - "
        "`cargo -vV` prints it as `host:`.")
endif()

set(DUSK_NODE_OUTPUT_DIRECTORY
    "${DUSK_NODE_CARGO_TARGET_DIR}/${DUSK_NODE_CARGO_TARGET}/${DUSK_NODE_PROFILE_DIRECTORY}")
set(DUSK_NODE_TARGET_ARGUMENTS
    --target "${DUSK_NODE_CARGO_TARGET}"
    --target-dir "${DUSK_NODE_CARGO_TARGET_DIR}")
set(DUSK_NODE_TRIPLE "${DUSK_NODE_CARGO_TARGET}")

string(TOLOWER "${DUSK_NODE_TRIPLE}" DUSK_NODE_TRIPLE)

file(STRINGS "${CMAKE_CURRENT_LIST_DIR}/lib/Cargo.toml" DUSK_NODE_FEATURE_LINES
     REGEX "^impl_[a-z0-9_]+[ \t]*=")
set(DUSK_NODE_IMPLS "")
foreach(feature_line IN LISTS DUSK_NODE_FEATURE_LINES)
    string(REGEX MATCH "^impl_([a-z0-9_]+)" _match "${feature_line}")
    list(APPEND DUSK_NODE_IMPLS "${CMAKE_MATCH_1}")
endforeach()
list(SORT DUSK_NODE_IMPLS)
list(JOIN DUSK_NODE_IMPLS ", " DUSK_NODE_IMPLS_READABLE)

if(NOT DUSK_NODE_IMPLS)
    message(FATAL_ERROR
        "No impl_* feature found in ${CMAKE_CURRENT_LIST_DIR}/lib/Cargo.toml")
endif()

if(DUSK_NODE_IMPL STREQUAL "")
    message(FATAL_ERROR
        "DUSK_NODE_IMPL is not set. It names the impl the node links, one of: "
        "${DUSK_NODE_IMPLS_READABLE}")
endif()

if(NOT DUSK_NODE_IMPL IN_LIST DUSK_NODE_IMPLS)
    message(FATAL_ERROR
        "DUSK_NODE_IMPL is '${DUSK_NODE_IMPL}', which is not one of: "
        "${DUSK_NODE_IMPLS_READABLE}")
endif()

set(DUSK_NODE_FEATURE_ARGUMENTS
    --no-default-features --features "impl_${DUSK_NODE_IMPL}")

separate_arguments(DUSK_NODE_CARGO_SUBCOMMAND_ARGUMENTS NATIVE_COMMAND
    "${DUSK_NODE_CARGO_SUBCOMMAND}")

if(NOT DUSK_NODE_CARGO_SUBCOMMAND_ARGUMENTS)
    message(FATAL_ERROR
        "DUSK_NODE_CARGO_SUBCOMMAND is empty. It names the cargo subcommand that "
        "builds the node, `build` by default.")
endif()

set(DUSK_NODE_RUSTFLAGS_ARGUMENTS "")
if(NOT DUSK_NODE_RUSTFLAGS STREQUAL "")
    separate_arguments(DUSK_NODE_RUSTFLAGS_LIST NATIVE_COMMAND "${DUSK_NODE_RUSTFLAGS}")
    set(DUSK_NODE_RUSTFLAGS_TOML "")
    foreach(flag IN LISTS DUSK_NODE_RUSTFLAGS_LIST)
        string(REPLACE "\\" "\\\\" flag "${flag}")
        string(REPLACE "\"" "\\\"" flag "${flag}")
        list(APPEND DUSK_NODE_RUSTFLAGS_TOML "\"${flag}\"")
    endforeach()
    list(JOIN DUSK_NODE_RUSTFLAGS_TOML ", " DUSK_NODE_RUSTFLAGS_TOML)
    set(DUSK_NODE_RUSTFLAGS_ARGUMENTS
        --config "build.rustflags=[${DUSK_NODE_RUSTFLAGS_TOML}]")
endif()

set(DUSK_NODE_ENVIRONMENT_ARGUMENTS
    "DUSK_NODE_INCLUDE_DIRECTORY=${DUSK_NODE_INCLUDE_DIRECTORY}")
if(NOT DUSK_NODE_INIT_SCRIPT STREQUAL "")
    if(DUSK_NODE_INIT_SCRIPT MATCHES "\n")
        message(FATAL_ERROR
            "DUSK_NODE_INIT_SCRIPT holds a line break, which the build cannot pass to "
            "cargo. Separate its commands with `;` instead.")
    endif()
    string(REPLACE ";" "$<SEMICOLON>" DUSK_NODE_INIT_SCRIPT_ESCAPED "${DUSK_NODE_INIT_SCRIPT}")
    list(APPEND DUSK_NODE_ENVIRONMENT_ARGUMENTS
        "DUSK_NODE_INIT_SCRIPT=${DUSK_NODE_INIT_SCRIPT_ESCAPED}")
endif()

if(DUSK_NODE_TRIPLE MATCHES "windows-msvc")
    set(DUSK_NODE_SYSTEM_LIBRARIES bcrypt advapi32 kernel32 ntdll userenv ws2_32 dbghelp)
    set(DUSK_NODE_LIBRARY_PREFIX "")
    set(DUSK_NODE_LIBRARY_SUFFIX ".lib")
    set(DUSK_NODE_EXECUTABLE_SUFFIX ".exe")
elseif(DUSK_NODE_TRIPLE MATCHES "linux-musl")
    set(DUSK_NODE_SYSTEM_LIBRARIES unwind c)
    set(DUSK_NODE_LIBRARY_PREFIX "lib")
    set(DUSK_NODE_LIBRARY_SUFFIX ".a")
    set(DUSK_NODE_EXECUTABLE_SUFFIX "")
elseif(DUSK_NODE_TRIPLE MATCHES "apple|darwin")
    set(DUSK_NODE_SYSTEM_LIBRARIES iconv System c m)
    set(DUSK_NODE_LIBRARY_PREFIX "lib")
    set(DUSK_NODE_LIBRARY_SUFFIX ".a")
    set(DUSK_NODE_EXECUTABLE_SUFFIX "")
elseif(DUSK_NODE_TRIPLE MATCHES "windows")
    set(DUSK_NODE_SYSTEM_LIBRARIES bcrypt advapi32 kernel32 ntdll userenv ws2_32 dbghelp)
    set(DUSK_NODE_LIBRARY_PREFIX "lib")
    set(DUSK_NODE_LIBRARY_SUFFIX ".a")
    set(DUSK_NODE_EXECUTABLE_SUFFIX ".exe")
elseif(DUSK_NODE_TRIPLE MATCHES "espidf|wasm|none|nuttx|vex|vxworks|qnx|trusty|sgx|l4re|solid|hermit|helenos|motor|vita")
    set(DUSK_NODE_SYSTEM_LIBRARIES "")
    set(DUSK_NODE_LIBRARY_PREFIX "lib")
    set(DUSK_NODE_LIBRARY_SUFFIX ".a")
    set(DUSK_NODE_EXECUTABLE_SUFFIX "")
else()
    set(DUSK_NODE_SYSTEM_LIBRARIES gcc_s util rt pthread m dl c)
    set(DUSK_NODE_LIBRARY_PREFIX "lib")
    set(DUSK_NODE_LIBRARY_SUFFIX ".a")
    set(DUSK_NODE_EXECUTABLE_SUFFIX "")
endif()

set(DUSK_NODE_LIBRARY_PATH
    "${CMAKE_CURRENT_BINARY_DIR}/lib/${DUSK_NODE_LIBRARY_PREFIX}dusk_node${DUSK_NODE_LIBRARY_SUFFIX}")
set(DUSK_NODE_EXECUTABLE_PATH
    "${CMAKE_CURRENT_BINARY_DIR}/bin/dusk_node${DUSK_NODE_EXECUTABLE_SUFFIX}")

# `cmake --build <dir> --target clean` also removes the cargo output.
set_property(DIRECTORY APPEND PROPERTY ADDITIONAL_CLEAN_FILES
    "${DUSK_NODE_CARGO_TARGET_DIR}" "${DUSK_NODE_INCLUDE_DIRECTORY}")

# ---- library ----

add_custom_target(dusk_node
    COMMAND "${CMAKE_COMMAND}" -E env ${DUSK_NODE_ENVIRONMENT_ARGUMENTS}
            "${DUSK_NODE_CARGO}" ${DUSK_NODE_CARGO_SUBCOMMAND_ARGUMENTS}
            --profile "${DUSK_NODE_CARGO_PROFILE}"
            --package dusk_node
            ${DUSK_NODE_FEATURE_ARGUMENTS}
            ${DUSK_NODE_TARGET_ARGUMENTS}
            ${DUSK_NODE_RUSTFLAGS_ARGUMENTS}
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
            "${DUSK_NODE_OUTPUT_DIRECTORY}/${DUSK_NODE_LIBRARY_PREFIX}dusk_node${DUSK_NODE_LIBRARY_SUFFIX}"
            "${DUSK_NODE_LIBRARY_PATH}"
    WORKING_DIRECTORY "${DUSK_NODE_WORKSPACE}"
    BYPRODUCTS "${DUSK_NODE_LIBRARY_PATH}"
    COMMENT "Building dusk_node with the ${DUSK_NODE_IMPL} impl"
    VERBATIM)

add_library(dusk::node STATIC IMPORTED GLOBAL)
add_dependencies(dusk::node dusk_node)
set_target_properties(dusk::node PROPERTIES
    IMPORTED_LOCATION "${DUSK_NODE_LIBRARY_PATH}"
    INTERFACE_INCLUDE_DIRECTORIES "${DUSK_NODE_INCLUDE_DIRECTORY}"
    INTERFACE_LINK_LIBRARIES "${DUSK_NODE_SYSTEM_LIBRARIES}")

# ---- executable ----

add_custom_target(dusk_node_bin
    COMMAND "${CMAKE_COMMAND}" -E env ${DUSK_NODE_ENVIRONMENT_ARGUMENTS}
            "${DUSK_NODE_CARGO}" ${DUSK_NODE_CARGO_SUBCOMMAND_ARGUMENTS}
            --profile "${DUSK_NODE_CARGO_PROFILE}"
            --package dusk_node_bin
            ${DUSK_NODE_FEATURE_ARGUMENTS}
            ${DUSK_NODE_TARGET_ARGUMENTS}
            ${DUSK_NODE_RUSTFLAGS_ARGUMENTS}
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
            "${DUSK_NODE_OUTPUT_DIRECTORY}/dusk_node${DUSK_NODE_EXECUTABLE_SUFFIX}"
            "${DUSK_NODE_EXECUTABLE_PATH}"
    WORKING_DIRECTORY "${DUSK_NODE_WORKSPACE}"
    BYPRODUCTS "${DUSK_NODE_EXECUTABLE_PATH}"
    COMMENT "Building dusk_node_bin with the ${DUSK_NODE_IMPL} impl"
    VERBATIM)

add_executable(dusk::node_bin IMPORTED GLOBAL)
add_dependencies(dusk::node_bin dusk_node_bin)
set_target_properties(dusk::node_bin PROPERTIES
    IMPORTED_LOCATION "${DUSK_NODE_EXECUTABLE_PATH}")