#
# Copyright 2018, Data61
# Commonwealth Scientific and Industrial Research Organisation (CSIRO)
# ABN 41 687 119 230.
#
# This software may be distributed and modified according to the terms of
# the BSD 2-Clause license. Note that NO WARRANTY is provided.
# See "LICENSE_BSD2.txt" for details.
#
# @TAG(DATA61_BSD)
#

include(${CMAKE_CURRENT_LIST_DIR}/settings.cmake)

cmake_minimum_required(VERSION 3.7.2)
project(bridge C ASM)

find_package(seL4 REQUIRED)
find_package(elfloader-tool REQUIRED)
find_package(musllibc REQUIRED)
find_package(util_libs REQUIRED)
find_package(seL4_libs REQUIRED)

sel4_import_kernel()
elfloader_import_project()

# This sets up environment build flags and imports musllibc and runtime libraries.
musllibc_setup_build_environment_with_sel4runtime()
sel4_import_libsel4()
util_libs_import_libraries()
sel4_libs_import_libraries()

include(cpio)
MakeCPIO(archive.o "${CMAKE_CURRENT_LIST_DIR}/../target/x86_64-sel4/release/zcboot-sel4")

add_executable(bridge archive.o main.c)

target_link_libraries(bridge
    sel4runtime sel4
    muslc utils
    sel4muslcsys sel4platsupport sel4utils sel4debug sel4allocman)

include(rootserver)
DeclareRootserver(bridge)

include(simulation)
GenerateSimulateScript()
