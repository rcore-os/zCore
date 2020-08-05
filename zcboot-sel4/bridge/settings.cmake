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
set(POLLY_DIR ${project_dir}/polly CACHE INTERNAL "")

include(application_settings)

# Deal with the top level target-triplet variables.
if(NOT BOARD)
    message(
        FATAL_ERROR
            "Please select a board to compile for."
    )
endif()

# Set arch and board specific kernel parameters here.
if(${BOARD} STREQUAL "pc")
    set(KernelArch "x86" CACHE STRING "" FORCE)
    set(KernelPlatform "pc99" CACHE STRING "" FORCE)
    if(${ARCH} STREQUAL "ia32")
        set(KernelSel4Arch "ia32" CACHE STRING "" FORCE)
    elseif(${ARCH} STREQUAL "x86_64")
        set(KernelSel4Arch "x86_64" CACHE STRING "" FORCE)
    else()
        message(FATAL_ERROR "Unsupported PC architecture ${ARCH}")
    endif()
elseif(${BOARD} STREQUAL "zynq7000")
    # Do a quick check and warn the user if they haven't set
    # -DARM/-DAARCH32/-DAARCH64.
    if(
        (NOT ARM)
        AND (NOT AARCH32)
        AND ((NOT CROSS_COMPILER_PREFIX) OR ("${CROSS_COMPILER_PREFIX}" STREQUAL ""))
    )
        message(
            WARNING
                "The target machine is an ARM machine. Unless you've defined -DCROSS_COMPILER_PREFIX, you may need to set one of:\n\t-DARM/-DAARCH32/-DAARCH64"
        )
    endif()

    set(KernelArch "arm" CACHE STRING "" FORCE)
    set(KernelSel4Arch "aarch32" CACHE STRING "" FORCE)
    set(KernelPlatform "zynq7000" CACHE STRING "" FORCE)
    ApplyData61ElfLoaderSettings(${KernelPlatform} ${KernelSel4Arch})
else()
    message(FATAL_ERROR "Unsupported board ${BOARD}.")
endif()

include(${project_dir}/seL4/configs/seL4Config.cmake)
set(CapDLLoaderMaxObjects 20000 CACHE STRING "" FORCE)
set(KernelRootCNodeSizeBits 16 CACHE STRING "")

# For the tutorials that do initialize the plat support serial printing they still
# just want to use the kernel debug putchar if it exists
set(LibSel4PlatSupportUseDebugPutChar true CACHE BOOL "" FORCE)

# Just let the regular abort spin without calling DebugHalt to prevent needless
# confusing output from the kernel for a tutorial
set(LibSel4MuslcSysDebugHalt FALSE CACHE BOOL "" FORCE)

# Only configure a single domain for the domain scheduler
set(KernelNumDomains 1 CACHE STRING "" FORCE)

# We must build the debug kernel because the tutorials rely on seL4_DebugPutChar
# and they don't initialize a platsupport driver.
ApplyCommonReleaseVerificationSettings(FALSE FALSE)

# We will attempt to generate a simulation script, so try and generate a simulation
# compatible configuration
ApplyCommonSimulationSettings(${KernelArch})
if(FORCE_IOMMU)
    set(KernelIOMMU ON CACHE BOOL "" FORCE)
endif()
