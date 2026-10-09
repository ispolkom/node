# Нативная часть полного туннеля: hev-socks5-tunnel (MIT) принимает пакеты из TUN и отдаёт их локальному SOCKS5-прокси приложения.
# Собираем через CMake (а не ndk-build): Flutter-плагин сам занимает путь CMake, и Gradle не принимает два нативных пути сразу.
cmake_minimum_required(VERSION 3.18)
project(hev-socks5-tunnel C ASM)

set(HEV ${CMAKE_CURRENT_SOURCE_DIR}/hev-socks5-tunnel)
set(TP ${HEV}/third-part)
set(CMAKE_C_STANDARD 11)
add_compile_options(-O3 -fPIC)
if(ANDROID_ABI STREQUAL "armeabi-v7a")
  add_compile_options(-mfpu=neon)
endif()

# yaml
file(GLOB_RECURSE YAML_SRC ${TP}/yaml/src/*.c)
add_library(yaml STATIC ${YAML_SRC})
target_include_directories(yaml PRIVATE ${TP}/yaml/src ${TP}/yaml/include)
target_compile_definitions(yaml PRIVATE YAML_VERSION_MAJOR=0 YAML_VERSION_MINOR=2 YAML_VERSION_PATCH=5 "YAML_VERSION_STRING=\"0.2.5\"")

# lwip
file(GLOB_RECURSE LWIP_SRC ${TP}/lwip/src/*.c)
add_library(lwip STATIC ${LWIP_SRC})
target_include_directories(lwip PRIVATE ${TP}/lwip/src/include ${TP}/lwip/src/ports/include)
target_compile_definitions(lwip PRIVATE FD_SET_DEFINED SOCKLEN_T_DEFINED)

# hev-task-system
file(GLOB_RECURSE HTS_SRC ${TP}/hev-task-system/src/*.c ${TP}/hev-task-system/src/*.S)
add_library(hev-task-system STATIC ${HTS_SRC})
target_include_directories(hev-task-system PRIVATE ${TP}/hev-task-system/src ${TP}/hev-task-system/include)
target_compile_definitions(hev-task-system PRIVATE
  ENABLE_STACK_OVERFLOW_DETECTION ENABLE_MEMALLOC_SLICE ENABLE_IO_SPLICE_SYSCALL
  CONFIG_STACK_BACKEND=STACK_MMAP CONFIG_STACK_OVERFLOW_DETECTION=1
  CONFIG_MEMALLOC_SLICE_ALIGN=16 CONFIG_MEMALLOC_SLICE_MAX_SIZE=4096 CONFIG_MEMALLOC_SLICE_MAX_COUNT=1000
  CONFIG_SCHED_CLOCK=CLOCK_NONE)
target_compile_options(hev-task-system PRIVATE -fvisibility=hidden)

# сам туннель (разделяемая библиотека для JNI)
file(GLOB_RECURSE HEV_SRC ${HEV}/src/*.c ${HEV}/src/*.S)
add_library(hev-socks5-tunnel SHARED ${HEV_SRC})
target_include_directories(hev-socks5-tunnel PRIVATE
  ${HEV}/src ${HEV}/src/misc ${HEV}/src/core/include
  ${TP}/yaml/include ${TP}/lwip/src/include ${TP}/lwip/src/ports/include ${TP}/hev-task-system/include)
target_compile_definitions(hev-socks5-tunnel PRIVATE FD_SET_DEFINED SOCKLEN_T_DEFINED ENABLE_LIBRARY "COMMIT_ID=\"07bea57\"")
target_link_libraries(hev-socks5-tunnel yaml lwip hev-task-system log)
target_link_options(hev-socks5-tunnel PRIVATE -Wl,-z,max-page-size=16384 -Wl,-z,common-page-size=16384)
