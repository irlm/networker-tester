cmake_minimum_required(VERSION 3.20)
project(alethabench-cpp LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# Release optimizations
if(NOT CMAKE_BUILD_TYPE)
    set(CMAKE_BUILD_TYPE Release)
endif()
set(CMAKE_CXX_FLAGS_RELEASE "-O3 -DNDEBUG")

# Dependencies
find_package(Boost REQUIRED COMPONENTS system)
find_package(OpenSSL REQUIRED)
find_package(Threads REQUIRED)

add_executable(server server.cpp)
target_link_libraries(server
    PRIVATE
        Boost::system
        OpenSSL::SSL
        OpenSSL::Crypto
        Threads::Threads
)
target_include_directories(server PRIVATE ${Boost_INCLUDE_DIRS})
