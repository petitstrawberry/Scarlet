cmake_minimum_required(VERSION 3.24)
project(ScarletCXXSupport VERSION 0.1.0 LANGUAGES C CXX)

if(NOT CMAKE_SYSTEM_NAME STREQUAL "Scarlet")
    message(FATAL_ERROR "Use a native Scarlet cross toolchain; host runtime builds are unsupported")
endif()
if(NOT CMAKE_SYSTEM_PROCESSOR MATCHES "^(aarch64|arm64)$")
    message(FATAL_ERROR "This companion currently implements the native AArch64 ABI")
endif()

set(SCARLET_LLVM_LIBC_SOURCE_DIR "" CACHE PATH "Pinned LLVM libc source directory, containing shared/ and src/")
if(NOT EXISTS "${SCARLET_LLVM_LIBC_SOURCE_DIR}/shared/str_to_float.h"
   OR NOT EXISTS "${SCARLET_LLVM_LIBC_SOURCE_DIR}/src/__support/str_to_float.h")
    message(FATAL_ERROR "SCARLET_LLVM_LIBC_SOURCE_DIR must name the SDK's pinned LLVM libc source checkout")
endif()

add_library(scarlet_cxx_support STATIC
    exit.cpp float-conversion.cpp locale.c locale-perror.c path-extensions.c
    temporary-files.c tmpfile.c rand.c scanf.cpp setjmp.cpp longjmp.cpp
    fenv-aarch64.c lrint.c math-round.c)
add_library(Scarlet::CXXSupport ALIAS scarlet_cxx_support)
set_target_properties(scarlet_cxx_support PROPERTIES
    EXPORT_NAME CXXSupport
    C_STANDARD 17 C_STANDARD_REQUIRED YES
    CXX_STANDARD 23 CXX_STANDARD_REQUIRED YES
    POSITION_INDEPENDENT_CODE ON)
target_include_directories(scarlet_cxx_support
    PUBLIC $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/../include>
           $<INSTALL_INTERFACE:include>
    PRIVATE "${SCARLET_LLVM_LIBC_SOURCE_DIR}")
target_compile_options(scarlet_cxx_support PRIVATE
    -ffunction-sections -fdata-sections -fno-stack-protector -mno-outline-atomics
    -Wall -Wextra -Werror
    $<$<COMPILE_LANGUAGE:CXX>:-fno-exceptions>
    $<$<COMPILE_LANGUAGE:CXX>:-fno-rtti>)

# Libraries can consume CXXSupport without replacing any executable entry.
# Final executables using the current native CRT opt in to this adapter.
add_library(scarlet_c_startup STATIC startup.c)
add_library(Scarlet::CStartup ALIAS scarlet_c_startup)
set_target_properties(scarlet_c_startup PROPERTIES EXPORT_NAME CStartup
    C_STANDARD 17 C_STANDARD_REQUIRED YES POSITION_INDEPENDENT_CODE ON)
target_compile_options(scarlet_c_startup PRIVATE
    -ffunction-sections -fdata-sections -fno-stack-protector
    -Wall -Wextra -Werror)
target_link_libraries(scarlet_c_startup PUBLIC scarlet_cxx_support)
target_link_options(scarlet_c_startup INTERFACE
    "LINKER:--wrap=main" "LINKER:-u,__wrap_main")

include(GNUInstallDirs)
include(CMakePackageConfigHelpers)
install(TARGETS scarlet_cxx_support scarlet_c_startup EXPORT ScarletCXXSupportTargets
    ARCHIVE DESTINATION ${CMAKE_INSTALL_LIBDIR})
install(DIRECTORY ../include/ DESTINATION ${CMAKE_INSTALL_INCLUDEDIR})
install(EXPORT ScarletCXXSupportTargets NAMESPACE Scarlet::
    DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/ScarletCXXSupport)
configure_package_config_file(cmake/ScarletCXXSupportConfig.cmake.in
    "${CMAKE_CURRENT_BINARY_DIR}/ScarletCXXSupportConfig.cmake"
    INSTALL_DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/ScarletCXXSupport)
write_basic_package_version_file(
    "${CMAKE_CURRENT_BINARY_DIR}/ScarletCXXSupportConfigVersion.cmake"
    VERSION ${PROJECT_VERSION} COMPATIBILITY SameMajorVersion)
install(FILES "${CMAKE_CURRENT_BINARY_DIR}/ScarletCXXSupportConfig.cmake"
    "${CMAKE_CURRENT_BINARY_DIR}/ScarletCXXSupportConfigVersion.cmake"
    DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/ScarletCXXSupport)
