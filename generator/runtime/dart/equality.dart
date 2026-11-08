// Hand-written, generic: value equality for lists/bytes inside generated types.

bool wabiEquals(Object? a, Object? b) {
  if (identical(a, b)) return true;
  if (a is List && b is List) {
    if (a.length != b.length) return false;
    for (var i = 0; i < a.length; i++) {
      if (!wabiEquals(a[i], b[i])) return false;
    }
    return true;
  }
  return a == b;
}

int wabiHash(Object? v) => v is List ? Object.hashAll(v.map(wabiHash)) : v.hashCode;
