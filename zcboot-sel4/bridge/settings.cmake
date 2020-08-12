include_guard(GLOBAL)

set(project_dir "${CMAKE_CURRENT_LIST_DIR}/../../../")
list(
    APPEND
        CMAKE_MODULE_PATH
        ${project_dir}/seL4
        ${project_dir}/seL4_tools/cmake-tool/helpers/
        ${project_dir}/seL4_tools/elfloader-tool/
        ${project_dir}/musllibc
        ${project_dir}/util_libs
        ${project_dir}/seL4_libs
        ${project_dir}/sel4runtime
        ${project_dir}/zCore/zcboot-sel4/bridge/
)

include(${project_dir}/seL4/configs/X64_verified.cmake)

set(KernelVerificationBuild OFF CACHE BOOL "" FORCE)
set(KernelPrinting ON CACHE BOOL "" FORCE)

set(KernelMaxNumBootinfoUntypedCaps 230 CACHE STRING "" FORCE)
