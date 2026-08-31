cmake_minimum_required(VERSION 3.20)

project(omasheets_lok_spike LANGUAGES CXX)

find_package(PkgConfig REQUIRED)
pkg_check_modules(GTK3 REQUIRED IMPORTED_TARGET gtk+-3.0)

find_path(
  LIBREOFFICEKIT_INCLUDE_DIR
  NAMES LibreOfficeKit/LibreOfficeKit.hxx
  PATHS /usr/include/libreoffice
)

if(NOT LIBREOFFICEKIT_INCLUDE_DIR)
  message(FATAL_ERROR "LibreOfficeKit headers were not found; install libreoffice-fresh-sdk or libreoffice-still-sdk")
endif()

find_library(
  LIBREOFFICEKIT_GTK_LIBRARY
  NAMES libreofficekitgtk
  PATHS /usr/lib
  REQUIRED
)

add_executable(omasheets-lok-render lok_render.cpp)
target_compile_features(omasheets-lok-render PRIVATE cxx_std_20)
target_compile_options(omasheets-lok-render PRIVATE -Wall -Wextra -Wpedantic -Werror)
target_include_directories(omasheets-lok-render PRIVATE "${LIBREOFFICEKIT_INCLUDE_DIR}")
target_link_libraries(omasheets-lok-render PRIVATE "${CMAKE_DL_LIBS}")

add_executable(omasheets-window window.cpp)
target_compile_features(omasheets-window PRIVATE cxx_std_20)
target_compile_options(omasheets-window PRIVATE -Wall -Wextra -Wpedantic -Werror)
target_include_directories(omasheets-window PRIVATE "${LIBREOFFICEKIT_INCLUDE_DIR}")
target_link_libraries(omasheets-window PRIVATE PkgConfig::GTK3 "${LIBREOFFICEKIT_GTK_LIBRARY}")

install(TARGETS omasheets-lok-render omasheets-window RUNTIME DESTINATION bin)
