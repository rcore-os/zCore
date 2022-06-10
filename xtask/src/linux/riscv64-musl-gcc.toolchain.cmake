set(CMAKE_SYSTEM_NAME "Linux")
set(CMAKE_SYSTEM_PROCESSOR "riscv64")

set(CMAKE_C_COMPILER  riscv64-linux-musl-gcc)
set(CMAKE_CXX_COMPILER riscv64-linux-musl-g++)

set(CMAKE_CXX_FLAGS ""    CACHE STRING "")
set(CMAKE_C_FLAGS ""    CACHE STRING "")

#set(CMAKE_CXX_FLAGS "-static -march=rv64gcvxthead -mabi=lp64v -pthread -D__riscv_vector_071")
#set(CMAKE_C_FLAGS "-static -march=rv64gcvxthead -mabi=lp64v -pthread -D__riscv_vector_071")

# To link ffmpeg libs
#set(CMAKE_PASS_TEST_FLAGS " -I/home/os/rust/aruco_demo/ffmpeg-5.0.1/build/install/include -L/home/os/rust/aruco_demo/ffmpeg-5.0.1/build/install/lib ")
set(CMAKE_LD_FFMPEG_FLAGS "-Wl,-rpath-link,/home/os/rust/aruco_demo/ffmpeg-5.0.1/build/install/lib")
set(CMAKE_EXE_LINKER_FLAGS "${CMAKE_EXE_LINKER_FLAGS} ${CMAKE_LD_FFMPEG_FLAGS}")

set(CMAKE_C_FLAGS "-march=rv64gc ${CMAKE_C_FLAGS} ${CMAKE_PASS_TEST_FLAGS}")
set(CMAKE_CXX_FLAGS "-march=rv64gc ${CXX_FLAGS} ${CMAKE_PASS_TEST_FLAGS}")
