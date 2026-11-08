# Builds the JNI shim of this package for Android (per ABI) — e.g. from an app module:
#
#   android { externalNativeBuild { cmake { path "<sdk-output>/java/jni/CMakeLists.txt" } } }
#
# The core itself (lib@LIBRARY@.so built for the target Android ABI) goes
# into src/main/jniLibs/<abi>/; the shim finds it at run time via dlopen, so the
# shim never has to be rebuilt when the core (or its language) changes.
cmake_minimum_required(VERSION 3.18)
project(wabi_jni C)

add_library(wabi_jni SHARED wabi_jni.c)
target_include_directories(wabi_jni PRIVATE ${CMAKE_CURRENT_LIST_DIR}/../../../abi)
set_target_properties(wabi_jni PROPERTIES C_STANDARD 11)
target_compile_options(wabi_jni PRIVATE -Wall -Wextra -Werror)
target_link_libraries(wabi_jni PRIVATE dl)   # JNI headers come from the NDK sysroot
