cmake_minimum_required(VERSION 3.20)

foreach(required IN ITEMS
        NETD_SOURCE_DIR
        NETD_ARTIFACT
        NETD_OUTPUT
        NETD_INPUT_FILES
        NETD_COMPILER
        NETD_COMPILER_ID
        NETD_COMPILER_VERSION
        NETD_COMPILER_TARGET
        NETD_COMPILER_FRONTEND_VARIANT
        NETD_LINKER
        NETD_BUILD_TYPE
        NETD_ANDROID_ABI
        NETD_ANDROID_PLATFORM
        NETD_CMAKE_GENERATOR
        NETD_CMAKE_VERSION
        NETD_ANDROID_NDK
        NETD_ANDROID_NDK_VERSION
        NETD_BUILD_JOBS)
    if(NOT DEFINED ${required})
        message(FATAL_ERROR "missing build metadata input: ${required}")
    endif()
endforeach()

function(json_escape input output)
    string(REPLACE "\\" "\\\\" value "${input}")
    string(REPLACE "\"" "\\\"" value "${value}")
    string(REPLACE "\n" "\\n" value "${value}")
    set(${output} "${value}" PARENT_SCOPE)
endfunction()

set(input_entries "")
foreach(input IN LISTS NETD_INPUT_FILES)
    if(NOT EXISTS "${input}")
        message(FATAL_ERROR "build metadata source is missing: ${input}")
    endif()
    file(RELATIVE_PATH relative "${NETD_SOURCE_DIR}" "${input}")
    file(SHA256 "${input}" digest)
    json_escape("${relative}" relative_json)
    string(APPEND input_entries "\n    \"${relative_json}\": \"${digest}\",")
endforeach()
string(REGEX REPLACE ",$" "" input_entries "${input_entries}")

if(NOT EXISTS "${NETD_ARTIFACT}")
    message(FATAL_ERROR "build artifact is missing: ${NETD_ARTIFACT}")
endif()
file(SHA256 "${NETD_ARTIFACT}" artifact_sha256)
file(SIZE "${NETD_ARTIFACT}" artifact_size)
file(RELATIVE_PATH artifact_relative "${NETD_SOURCE_DIR}" "${NETD_ARTIFACT}")

json_escape("${NETD_COMPILER}" compiler_json)
json_escape("${NETD_COMPILER_ID}" compiler_id_json)
json_escape("${NETD_COMPILER_VERSION}" compiler_version_json)
json_escape("${NETD_COMPILER_TARGET}" compiler_target_json)
json_escape("${NETD_COMPILER_FRONTEND_VARIANT}" compiler_variant_json)
json_escape("${NETD_LINKER}" linker_json)
json_escape("${NETD_BUILD_TYPE}" build_type_json)
json_escape("${NETD_ANDROID_ABI}" abi_json)
json_escape("${NETD_ANDROID_PLATFORM}" platform_json)
json_escape("${NETD_CMAKE_GENERATOR}" generator_json)
json_escape("${NETD_CMAKE_VERSION}" cmake_version_json)
json_escape("${NETD_ANDROID_NDK}" ndk_json)
json_escape("${NETD_ANDROID_NDK_VERSION}" ndk_version_json)
json_escape("${artifact_relative}" artifact_relative_json)

set(metadata "{
  \"schema\": 1,
  \"component\": \"user/android-runtime/network\",
  \"generated_by\": \"CMakeLists.txt:netd_updatable POST_BUILD\",
  \"build\": {
    \"system\": \"CMake\",
    \"cmake_version\": \"${cmake_version_json}\",
    \"generator\": \"${generator_json}\",
    \"type\": \"${build_type_json}\",
    \"jobs\": ${NETD_BUILD_JOBS},
    \"android_abi\": \"${abi_json}\",
    \"android_platform\": \"${platform_json}\",
    \"android_ndk\": \"${ndk_json}\",
    \"android_ndk_version\": \"${ndk_version_json}\"
  },
  \"compiler\": {
    \"path\": \"${compiler_json}\",
    \"cmake_id\": \"${compiler_id_json}\",
    \"version\": \"${compiler_version_json}\",
    \"target\": \"${compiler_target_json}\",
    \"frontend_variant\": \"${compiler_variant_json}\",
    \"linker\": \"${linker_json}\"
  },
  \"inputs_sha256\": {${input_entries}
  },
  \"artifact\": {
    \"path\": \"${artifact_relative_json}\",
    \"size_bytes\": ${artifact_size},
    \"sha256\": \"${artifact_sha256}\",
    \"soname\": \"libnetd_updatable.so\",
    \"exports_expected\": [
      \"libnetd_updatable_init@@LIBNETD_UPDATABLE\",
      \"libnetd_updatable_tagSocket@@LIBNETD_UPDATABLE\",
      \"libnetd_updatable_untagSocket@@LIBNETD_UPDATABLE\"
    ]
  },
  \"provider_contract\": {
    \"native_capability_query\": \"0x53430000+922\",
    \"supported_operations_mask\": \"0xfe\",
    \"quota2_backend\": true,
    \"interface_budget_backend\": true,
    \"uid_socket_tagging\": {
      \"supported\": false,
      \"error\": \"EOPNOTSUPP\"
    },
    \"claims_bpf_support\": false,
    \"sets_bpf_progs_loaded\": false,
    \"packet_attachments\": false
  },
  \"export_verification\": \"parent readelf step\"
}
")

get_filename_component(output_directory "${NETD_OUTPUT}" DIRECTORY)
file(MAKE_DIRECTORY "${output_directory}")
file(WRITE "${NETD_OUTPUT}" "${metadata}")

