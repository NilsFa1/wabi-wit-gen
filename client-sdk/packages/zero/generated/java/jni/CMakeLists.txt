# Copied by wabi-wit-gen from generator/runtime — DO NOT EDIT; change the source there.
# Builds the JNI shim of this package for Android (per ABI) — e.g. from an app module:
#
#   android { externalNativeBuild { cmake { path "<package>/generated/java/jni/CMakeLists.txt" } } }
#
# The core itself (libwabi_client.so from scripts/build-native.sh android) goes
# into src/main/jniLibs/<abi>/; the shim finds it at run time via dlopen, so the
# shim never has to be rebuilt when the core (or its language) changes.
cmake_minimum_required(VERSION 3.18)
project(gematik_zero_jni C)

add_library(gematik_zero_jni SHARED gematik_zero_jni.c)
target_include_directories(gematik_zero_jni PRIVATE ${CMAKE_CURRENT_LIST_DIR}/../../../../../abi)
set_target_properties(gematik_zero_jni PROPERTIES C_STANDARD 11)
target_compile_options(gematik_zero_jni PRIVATE -Wall -Wextra -Werror)
target_link_libraries(gematik_zero_jni PRIVATE dl)   # JNI headers come from the NDK sysroot
