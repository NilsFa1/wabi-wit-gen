# Runtime correctness fixes

## Host callback lifetime

The Dart backend retains one NativeCallable per backend for the isolate's
lifetime. Registering replaces its dispatcher; unregistering clears the
dispatcher and removes the core registration without closing the trampoline.
A core that already copied the pointer still gets a reply (errUnsupported when
no dispatcher is installed). Each call captures the dispatcher when it enters
Dart, so an asynchronous response remains bound to its original implementation.

The owning isolate must outlive all core operations and outstanding callbacks.
This change does not make terminating an isolate while native work is running
safe. Swift/C#/Java already use callbacks with static/process lifetime and do
not free a trampoline on unregister.

## Stream pulls

Cancelling a Go/Rust pull signals its worker. Cancellation becomes observable
only after that worker has restored any dequeued item and released the busy
flag. Completion still wins if it committed first. Future-state publication,
item restoration and busy release precede notification; callbacks execute
outside locks and can immediately request the next item.

The ABI is unchanged. Runtime regression tests force cancellation after dequeue,
check FIFO delivery, and cover restarting a pull on completion.

## OCSP trust generations

Each verifier atomically publishes the parsed TSL together with a private
OCSP cache. Changing the TSL creates a fresh generation. In-flight checks retain
their old generation and cannot populate the new cache. Parsed TSLs may still
be shared between verifiers; HTTP outcomes are not shared. Hit/miss counters
span generations, while entry counts describe the current cache.
