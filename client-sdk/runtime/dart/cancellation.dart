// Hand-written, generic. Dart futures have no built-in cancellation, so async
// SDK methods accept an optional token (like C#'s CancellationToken).
import 'dart:async';

import 'errors.dart';

/// Cancels one or more in-flight SDK operations. Cancellation is forwarded to
/// the native core (acme_future_cancel); the awaiting future then completes
/// with [AcmeCancelledException] — unless the operation had already finished
/// ("completion wins", ABI.md §4.1).
final class CancellationToken {
  CancellationToken();

  /// A token that is cancelled after [timeout].
  factory CancellationToken.timeout(Duration timeout) {
    final t = CancellationToken();
    t._timer = Timer(timeout, t.cancel);
    return t;
  }

  bool _cancelled = false;
  Timer? _timer;
  final _listeners = <void Function()>[];

  bool get isCancelled => _cancelled;

  void cancel() {
    if (_cancelled) return;
    _cancelled = true;
    _timer?.cancel();
    final ls = List.of(_listeners);
    _listeners.clear();
    for (final l in ls) {
      l();
    }
  }

  void throwIfCancelled() {
    if (_cancelled) throw AcmeCancelledException();
  }

  /// Registers [listener]; returns a function that unregisters it.
  void Function() register(void Function() listener) {
    if (_cancelled) {
      listener();
      return () {};
    }
    _listeners.add(listener);
    return () => _listeners.remove(listener);
  }
}
