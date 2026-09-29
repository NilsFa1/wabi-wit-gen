// Hand-written, generic: value equality for lists/bytes inside generated types.

bool acmeEquals(Object? a, Object? b) {
  if (identical(a, b)) return true;
  if (a is List && b is List) {
    if (a.length != b.length) return false;
    for (var i = 0; i < a.length; i++) {
      if (!acmeEquals(a[i], b[i])) return false;
    }
    return true;
  }
  return a == b;
}

int acmeHash(Object? v) => v is List ? Object.hashAll(v.map(acmeHash)) : v.hashCode;
