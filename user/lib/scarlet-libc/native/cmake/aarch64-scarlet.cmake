# Bootstrap toolchain for building the companion before the SDK is installed.
# Set CMAKE_C_COMPILER/CMAKE_CXX_COMPILER to raw Clang or a native SDK driver.
set(CMAKE_SYSTEM_NAME Scarlet)
set(CMAKE_SYSTEM_PROCESSOR aarch64)
set(CMAKE_TRY_COMPILE_TARGET_TYPE STATIC_LIBRARY)
list(PREPEND CMAKE_MODULE_PATH "${CMAKE_CURRENT_LIST_DIR}")
set(CMAKE_C_COMPILER_TARGET aarch64-unknown-none-elf)
set(CMAKE_CXX_COMPILER_TARGET aarch64-unknown-none-elf)

if(NOT CMAKE_C_COMPILER)
    find_program(CMAKE_C_COMPILER NAMES clang REQUIRED)
endif()
if(NOT CMAKE_CXX_COMPILER)
    find_program(CMAKE_CXX_COMPILER NAMES clang++ REQUIRED)
endif()
execute_process(COMMAND "${CMAKE_C_COMPILER}" -print-resource-dir
    OUTPUT_VARIABLE _scarlet_clang_resource OUTPUT_STRIP_TRAILING_WHITESPACE
    COMMAND_ERROR_IS_FATAL ANY)
if(NOT EXISTS "${_scarlet_clang_resource}/include/stddef.h")
    message(FATAL_ERROR "Clang's builtin header directory is unavailable")
endif()
get_filename_component(_scarlet_c_headers "${CMAKE_CURRENT_LIST_DIR}/../../include" ABSOLUTE)
set(CMAKE_C_FLAGS_INIT "-nostdinc -isystem \"${_scarlet_clang_resource}/include\" -isystem \"${_scarlet_c_headers}\" -D__scarlet__=1")
set(CMAKE_CXX_FLAGS_INIT "${CMAKE_C_FLAGS_INIT} -nostdinc++")
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
