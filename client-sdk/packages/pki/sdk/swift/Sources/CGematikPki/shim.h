// Exposes the Wabi native ABI of package gematik:pki to Swift: the generic
// types (wabi_abi.h) and this package's prefixed symbols (gematik_pki_*).
// The runtime (Generated/Runtime) imports this module as `CGematikPki`; Generated/NativeAbi.swift
// maps the prefixed symbols back to the generic names the runtime uses.
#include "../../../../../../abi/wabi_abi.h"
#include "../../../../generated/c/gematik_pki.h"
