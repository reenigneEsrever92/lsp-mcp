cmake_minimum_required(VERSION 3.20)
project(greeter LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

add_executable(greeter src/main.cpp src/greeter.cpp)
target_include_directories(greeter PRIVATE include)
target_compile_options(greeter PRIVATE -Wall -Wextra)
