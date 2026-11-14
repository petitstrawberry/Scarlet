cmake_minimum_required(VERSION 3.20)

project(scarlet_libnetd_updatable LANGUAGES C CXX)
set(NETD_BUILD_JOBS 2 CACHE STRING "Parallel build jobs recorded in metadata")

set(SOURCE_PINS "${CMAKE_CURRENT_SOURCE_DIR}/SOURCE_PINS.json")
set(MANIFEST_HELPER "${CMAKE_CURRENT_SOURCE_DIR}/cmake/WriteBuildMetadata.cmake")
file(READ "${SOURCE_PINS}" source_pins_json)
string(JSON source_pins_schema GET "${source_pins_json}" schema)
if(NOT source_pins_schema EQUAL 1)
    message(FATAL_ERROR "SOURCE_PINS.json schema must be 1")
endif()
foreach(pin IN ITEMS public_header version_script networkstats_header networkstats_types)
    string(JSON pinned_path GET "${source_pins_json}" pins ${pin} local_path)
    string(JSON pinned_sha256 GET "${source_pins_json}" pins ${pin} sha256)
    set(pinned_file "${CMAKE_CURRENT_SOURCE_DIR}/${pinned_path}")
    if(NOT EXISTS "${pinned_file}")
        message(FATAL_ERROR "Pinned source is missing: ${pinned_file}")
    endif()
    file(SHA256 "${pinned_file}" actual_sha256)
    if(NOT actual_sha256 STREQUAL pinned_sha256)
        message(FATAL_ERROR
            "Pinned source hash mismatch for ${pinned_path}: expected ${pinned_sha256}, got ${actual_sha256}")
    endif()
endforeach()

add_library(networkstats SHARED src/NetworkStats.cpp src/NetworkStatsBackend.c)
target_include_directories(networkstats PRIVATE include src)
target_compile_features(networkstats PRIVATE cxx_std_17)
target_compile_definitions(networkstats PRIVATE _GNU_SOURCE)
target_compile_options(networkstats PRIVATE -O2 -Wall -Wextra -Werror -g0
    -ffile-prefix-map=${CMAKE_CURRENT_SOURCE_DIR}=.)
set_target_properties(networkstats PROPERTIES C_VISIBILITY_PRESET hidden
    CXX_VISIBILITY_PRESET hidden VISIBILITY_INLINES_HIDDEN ON)
if(ANDROID)
    if(NOT EXISTS "${SCARLET_PLATFORM_LIBCXX}")
        message(FATAL_ERROR "SCARLET_PLATFORM_LIBCXX must select the pinned AOSP libc++.so")
    endif()
    target_compile_definitions(networkstats PRIVATE _LIBCPP_ABI_NAMESPACE=__1)
    target_compile_options(networkstats PRIVATE
        $<$<COMPILE_LANGUAGE:CXX>:-fno-exceptions> $<$<COMPILE_LANGUAGE:CXX>:-fno-rtti>)
    target_link_options(networkstats PRIVATE -nostdlib++ -Wl,--no-undefined
        -Wl,--version-script=${CMAKE_CURRENT_SOURCE_DIR}/libnetworkstats.map.txt)
    target_link_libraries(networkstats PRIVATE "${SCARLET_PLATFORM_LIBCXX}")
endif()
set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS
    "${SOURCE_PINS}"
    "${MANIFEST_HELPER}"
)

set(NETD_ANDROID_NDK_VERSION "${CMAKE_ANDROID_NDK_VERSION}")
if(NOT NETD_ANDROID_NDK_VERSION AND CMAKE_ANDROID_NDK)
    get_filename_component(NETD_ANDROID_NDK_VERSION "${CMAKE_ANDROID_NDK}" NAME)
endif()

add_library(netd_updatable SHARED
    src/Backend.c
    src/Provider.c
)

set_target_properties(netd_updatable PROPERTIES
    OUTPUT_NAME netd_updatable
    C_VISIBILITY_PRESET hidden
    POSITION_INDEPENDENT_CODE ON
)

target_compile_features(netd_updatable PRIVATE c_std_11)
target_compile_definitions(netd_updatable PRIVATE _GNU_SOURCE)
target_compile_options(netd_updatable PRIVATE
    -O2
    -Wall
    -Wextra
    -Werror
    -fvisibility=hidden
    -g0
    -ffile-prefix-map=${CMAKE_CURRENT_SOURCE_DIR}=.
)
target_include_directories(netd_updatable PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/include
    ${CMAKE_CURRENT_SOURCE_DIR}/src
)
target_link_options(netd_updatable PRIVATE
    -Wl,--build-id=sha1
    -Wl,--fatal-warnings
    -Wl,--no-undefined
    -Wl,--strip-debug
    -Wl,-soname,libnetd_updatable.so
    -Wl,--version-script=${CMAKE_CURRENT_SOURCE_DIR}/libnetd_updatable.map.txt
)

set(BUILD_METADATA_INPUTS
    "${CMAKE_CURRENT_SOURCE_DIR}/CMakeLists.txt"
    "${CMAKE_CURRENT_SOURCE_DIR}/Makefile"
    "${SOURCE_PINS}"
    "${MANIFEST_HELPER}"
    "${CMAKE_CURRENT_SOURCE_DIR}/include/NetdUpdatablePublic.h"
    "${CMAKE_CURRENT_SOURCE_DIR}/libnetd_updatable.map.txt"
    "${CMAKE_CURRENT_SOURCE_DIR}/src/Backend.c"
    "${CMAKE_CURRENT_SOURCE_DIR}/src/Backend.h"
    "${CMAKE_CURRENT_SOURCE_DIR}/src/Provider.c"
    "${CMAKE_CURRENT_SOURCE_DIR}/src/NetworkStats.cpp"
    "${CMAKE_CURRENT_SOURCE_DIR}/src/NetworkStatsBackend.c"
    "${CMAKE_CURRENT_SOURCE_DIR}/src/NetworkStatsBackend.h"
    "${CMAKE_CURRENT_SOURCE_DIR}/include/NetworkStatsPublic.h"
    "${CMAKE_CURRENT_SOURCE_DIR}/include/upstream/BpfNetworkStats.h"
    "${CMAKE_CURRENT_SOURCE_DIR}/include/upstream/bpf_shared.h"
    "${CMAKE_CURRENT_SOURCE_DIR}/libnetworkstats.map.txt"
)
add_custom_target(network_build_metadata ALL
    DEPENDS netd_updatable networkstats
    COMMAND "${CMAKE_COMMAND}"
        "-DNETD_SOURCE_DIR=${CMAKE_CURRENT_SOURCE_DIR}"
        "-DNETD_ARTIFACT=$<TARGET_FILE:netd_updatable>"
        "-DNETD_STATS_ARTIFACT=$<TARGET_FILE:networkstats>"
        "-DNETD_OUTPUT=${CMAKE_CURRENT_BINARY_DIR}/build.json"
        "-DNETD_INPUT_FILES=${BUILD_METADATA_INPUTS}"
        "-DNETD_COMPILER=${CMAKE_C_COMPILER}"
        "-DNETD_CXX_COMPILER=${CMAKE_CXX_COMPILER}"
        "-DNETD_PLATFORM_LIBCXX=${SCARLET_PLATFORM_LIBCXX}"
        "-DNETD_COMPILER_ID=${CMAKE_C_COMPILER_ID}"
        "-DNETD_COMPILER_VERSION=${CMAKE_C_COMPILER_VERSION}"
        "-DNETD_COMPILER_TARGET=${CMAKE_C_COMPILER_TARGET}"
        "-DNETD_COMPILER_FRONTEND_VARIANT=${CMAKE_C_COMPILER_FRONTEND_VARIANT}"
        "-DNETD_LINKER=${CMAKE_LINKER}"
        "-DNETD_BUILD_TYPE=${CMAKE_BUILD_TYPE}"
        "-DNETD_ANDROID_ABI=${ANDROID_ABI}"
        "-DNETD_ANDROID_PLATFORM=${ANDROID_PLATFORM}"
        "-DNETD_CMAKE_GENERATOR=${CMAKE_GENERATOR}"
        "-DNETD_CMAKE_VERSION=${CMAKE_VERSION}"
        "-DNETD_ANDROID_NDK=${CMAKE_ANDROID_NDK}"
        "-DNETD_ANDROID_NDK_VERSION=${NETD_ANDROID_NDK_VERSION}"
        "-DNETD_BUILD_JOBS=${NETD_BUILD_JOBS}"
        -P "${MANIFEST_HELPER}"
    VERBATIM
)

install(TARGETS netd_updatable
    LIBRARY DESTINATION lib64
)
install(TARGETS networkstats LIBRARY DESTINATION lib64)
install(FILES include/NetdUpdatablePublic.h
    DESTINATION include
)

option(NETD_UPDATABLE_BUILD_TESTS "Build the host-side provider tests" OFF)
if(NETD_UPDATABLE_BUILD_TESTS)
    find_package(Threads REQUIRED)

    add_executable(netd_updatable_provider_test
        src/Provider.c
        tests/StubBackend.c
        tests/ProviderTest.c
    )
    set_target_properties(netd_updatable_provider_test PROPERTIES
        C_VISIBILITY_PRESET hidden
    )
    target_compile_features(netd_updatable_provider_test PRIVATE c_std_11)
    target_compile_definitions(netd_updatable_provider_test PRIVATE _GNU_SOURCE)
    target_compile_options(netd_updatable_provider_test PRIVATE
        -O2
        -Wall
        -Wextra
        -Werror
    )
    target_include_directories(netd_updatable_provider_test PRIVATE
        ${CMAKE_CURRENT_SOURCE_DIR}/include
        ${CMAKE_CURRENT_SOURCE_DIR}/src
    )
    target_link_libraries(netd_updatable_provider_test PRIVATE Threads::Threads)

    enable_testing()
    add_test(NAME netd_updatable_provider_test
        COMMAND netd_updatable_provider_test
    )
    add_executable(networkstats_test src/NetworkStats.cpp tests/NetworkStatsTest.cpp)
    target_include_directories(networkstats_test PRIVATE include src)
    target_compile_features(networkstats_test PRIVATE cxx_std_17)
    target_compile_options(networkstats_test PRIVATE -O2 -Wall -Wextra -Werror)
    add_test(NAME networkstats_test COMMAND networkstats_test)

    add_executable(networkstats_backend_test
        src/NetworkStatsBackend.c
        tests/NetworkStatsBackendTest.c
    )
    target_include_directories(networkstats_backend_test PRIVATE include src)
    target_compile_features(networkstats_backend_test PRIVATE c_std_11)
    target_compile_definitions(networkstats_backend_test PRIVATE _GNU_SOURCE)
    target_compile_options(networkstats_backend_test PRIVATE -O2 -Wall -Wextra -Werror)
    if(APPLE)
        target_compile_options(networkstats_backend_test PRIVATE -Wno-deprecated-declarations)
    endif()
    add_test(NAME networkstats_backend_test COMMAND networkstats_backend_test)
endif()

