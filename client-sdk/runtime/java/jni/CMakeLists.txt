# Builds the generic JNI shim for Android (per ABI) — e.g. from an app module:
#
#   android { externalNativeBuild { cmake { path "<client-sdk>/runtime/java/jni/CMakeLists.txt" } } }
#
# The core itself (libacme_client.so from scripts/build-native.sh android) goes
# into src/main/jniLibs/<abi>/; the shim finds it at run time via dlopen, so the
# shim never has to be rebuilt when the core (or its language) changes.
cmake_minimum_required(VERSION 3.18)
project(acme_jni C)

add_library(acme_jni SHARED acme_jni.c)
target_include_directories(acme_jni PRIVATE ${CMAKE_CURRENT_LIST_DIR}/../../../abi)
set_target_properties(acme_jni PROPERTIES C_STANDARD 11)
target_compile_options(acme_jni PRIVATE -Wall -Wextra -Werror)
target_link_libraries(acme_jni PRIVATE dl)   # JNI headers come from the NDK sysroot
