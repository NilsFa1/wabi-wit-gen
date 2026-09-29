// Package acmeabi is the hand-written, contract-agnostic runtime for Go cores
// implementing the Acme native ABI (abi/acme_abi.h, ABI.md).
//
// A Go core consists of three parts:
//
//   - this package: handles, futures, streams, payload codec, host calls,
//     and the generic implementation of every ABI function,
//   - acme_generated.go (from acme-wit-gen): contract types, codec, the
//     interfaces the core implements, stubs for host imports, the method-id
//     dispatcher and the //export wrappers under the package symbol prefix,
//   - the hand-written business logic implementing the generated interfaces.
//
// The API uses only Go types (uint64, []byte, unsafe.Pointer): cgo's C types
// are package-scoped and cannot be shared with the generated package.
package acmeabi
