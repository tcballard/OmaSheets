cmake_minimum_required(VERSION 3.20)

project(omasheets_lok_spike LANGUAGES CXX)

find_path(
  LIBREOFFICEKIT_INCLUDE_DIR
  NAMES LibreOfficeKit/LibreOfficeKit.hxx
  PATHS /usr/include/libreoffice
)

if(NOT LIBREOFFICEKIT_INCLUDE_DIR)
  message(FATAL_ERROR "LibreOfficeKit headers were not found; install libreoffice-fresh or libreoffice-still")
endif()

add_executable(omasheets-lok-render lok_render.cpp)
target_compile_features(omasheets-lok-render PRIVATE cxx_std_20)
target_compile_options(omasheets-lok-render PRIVATE -Wall -Wextra -Wpedantic -Werror)
target_include_directories(omasheets-lok-render PRIVATE "${LIBREOFFICEKIT_INCLUDE_DIR}")
target_link_libraries(omasheets-lok-render PRIVATE "${CMAKE_DL_LIBS}")

install(TARGETS omasheets-lok-render RUNTIME DESTINATION bin)
