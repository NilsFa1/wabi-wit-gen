// Exposes the Wabi native ABI of package gematik:zero to Swift: the generic
// types (wabi_abi.h) and this package's prefixed symbols (gematik_zero_*).
// The runtime (Generated/Runtime) imports this module as `CGematikZero`; Generated/NativeAbi.swift
// maps the prefixed symbols back to the generic names the runtime uses.
#include "../../../../../../abi/wabi_abi.h"
#include "../../../../generated/c/gematik_zero.h"
