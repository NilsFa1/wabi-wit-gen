// Hand-written, generic: payload encoding v1 (MessagePack subset, ABI.md §5).
import 'dart:convert';
import 'dart:typed_data';

import 'errors.dart';

final class MsgPackWriter {
  final _b = BytesBuilder(copy: false);
  final _scratch = ByteData(8);

  Uint8List takeBytes() => _b.takeBytes();

  void _byte(int v) => _b.addByte(v);
  void _be(int v, int n) {
    for (var i = n - 1; i >= 0; i--) {
      _b.addByte((v >> (8 * i)) & 0xff);
    }
  }

  void writeNil() => _byte(0xc0);
  void writeBool(bool v) => _byte(v ? 0xc3 : 0xc2);

  /// Unsigned integer. Values are Dart ints; u64 values above 2^63-1 are passed
  /// as their (negative) two's-complement bit pattern.
  void writeUint(int v) {
    if (v >= 0 && v < 0x80) {
      _byte(v);
    } else if (v >= 0 && v <= 0xff) {
      _byte(0xcc);
      _be(v, 1);
    } else if (v >= 0 && v <= 0xffff) {
      _byte(0xcd);
      _be(v, 2);
    } else if (v >= 0 && v <= 0xffffffff) {
      _byte(0xce);
      _be(v, 4);
    } else {
      _byte(0xcf);
      _be(v, 8);
    }
  }

  void writeInt(int v) {
    if (v >= 0) {
      writeUint(v);
    } else if (v >= -32) {
      _byte(v & 0xff);
    } else if (v >= -0x80) {
      _byte(0xd0);
      _be(v, 1);
    } else if (v >= -0x8000) {
      _byte(0xd1);
      _be(v, 2);
    } else if (v >= -0x80000000) {
      _byte(0xd2);
      _be(v, 4);
    } else {
      _byte(0xd3);
      _be(v, 8);
    }
  }

  void writeFloat32(double v) {
    _byte(0xca);
    _scratch.setFloat32(0, v);
    _b.add(_scratch.buffer.asUint8List(0, 4));
  }

  void writeFloat64(double v) {
    _byte(0xcb);
    _scratch.setFloat64(0, v);
    _b.add(_scratch.buffer.asUint8List(0, 8));
  }

  void _header(int n, int? fixBase, int fixMax, int? m8, int m16, int m32) {
    if (fixBase != null && n <= fixMax) {
      _byte(fixBase | n);
    } else if (m8 != null && n <= 0xff) {
      _byte(m8);
      _byte(n);
    } else if (n <= 0xffff) {
      _byte(m16);
      _be(n, 2);
    } else {
      _byte(m32);
      _be(n, 4);
    }
  }

  /// UTF-8. Dart strings are UTF-16: unpaired surrogates cannot be encoded and
  /// throw [ArgumentError] before anything crosses the ABI.
  void writeString(String s) {
    _checkSurrogates(s);
    final bytes = utf8.encode(s);
    _header(bytes.length, 0xa0, 31, 0xd9, 0xda, 0xdb);
    _b.add(bytes);
  }

  void writeBinary(Uint8List v) {
    _header(v.length, null, 0, 0xc4, 0xc5, 0xc6);
    _b.add(v);
  }

  void writeArrayHeader(int n) => _header(n, 0x90, 15, null, 0xdc, 0xdd);
  void writeMapHeader(int n) => _header(n, 0x80, 15, null, 0xde, 0xdf);

  static void _checkSurrogates(String s) {
    for (var i = 0; i < s.length; i++) {
      final c = s.codeUnitAt(i);
      if (c >= 0xd800 && c <= 0xdbff) {
        if (i + 1 < s.length) {
          final d = s.codeUnitAt(i + 1);
          if (d >= 0xdc00 && d <= 0xdfff) {
            i++;
            continue;
          }
        }
        throw ArgumentError.value(s, 'string', 'unpaired surrogate at index $i');
      } else if (c >= 0xdc00 && c <= 0xdfff) {
        throw ArgumentError.value(s, 'string', 'unpaired surrogate at index $i');
      }
    }
  }
}

final class MsgPackReader {
  MsgPackReader(this._buf) : _data = ByteData.sublistView(_buf);

  final Uint8List _buf;
  final ByteData _data;
  int _pos = 0;
  static const _maxDepth = 64;

  int get remaining => _buf.length - _pos;

  static WabiProtocolException _eof() => WabiProtocolException('unexpected end of payload');
  static WabiProtocolException _unexpected(String what, int m) =>
      WabiProtocolException('expected $what, found marker 0x${m.toRadixString(16)}');

  int _peek() => _pos < _buf.length ? _buf[_pos] : throw _eof();
  int _next() => _pos < _buf.length ? _buf[_pos++] : throw _eof();

  void _need(int n) {
    if (n < 0 || n > remaining) throw WabiProtocolException('length exceeds payload');
  }

  int _be(int n) {
    _need(n);
    var v = 0;
    for (var i = 0; i < n; i++) {
      v = (v << 8) | _buf[_pos++];
    }
    return v;
  }

  Uint8List _take(int n) {
    _need(n);
    final s = Uint8List.sublistView(_buf, _pos, _pos + n);
    _pos += n;
    return s;
  }

  bool tryReadNil() {
    if (_peek() != 0xc0) return false;
    _pos++;
    return true;
  }

  void readNil() {
    final m = _next();
    if (m != 0xc0) throw _unexpected('nil', m);
  }

  bool readBool() => switch (_next()) {
        0xc2 => false,
        0xc3 => true,
        final m => throw _unexpected('bool', m),
      };

  /// Returns (negative?, value). For uint64 values >= 2^63 the value is the
  /// two's-complement bit pattern (negative Dart int) with negative == false.
  (bool, int) _integer() {
    final m = _next();
    if (m <= 0x7f) return (false, m);
    if (m >= 0xe0) return (true, m - 0x100);
    switch (m) {
      case 0xcc:
        return (false, _be(1));
      case 0xcd:
        return (false, _be(2));
      case 0xce:
        return (false, _be(4));
      case 0xcf:
        return (false, _be(8));
      case 0xd0:
        final v = _be(1).toSigned(8);
        return (v < 0, v);
      case 0xd1:
        final v = _be(2).toSigned(16);
        return (v < 0, v);
      case 0xd2:
        final v = _be(4).toSigned(32);
        return (v < 0, v);
      case 0xd3:
        final v = _be(8);
        return (v < 0, v);
      default:
        throw _unexpected('integer', m);
    }
  }

  /// Unsigned 64-bit value (bit pattern for values >= 2^63).
  int readUint() {
    final (neg, v) = _integer();
    if (neg) throw WabiProtocolException('negative value for unsigned integer');
    return v;
  }

  int readUintMax(int max) {
    final v = readUint();
    if (v < 0 || v > max) throw WabiProtocolException('integer $v out of range');
    return v;
  }

  int readInt() {
    final (neg, v) = _integer();
    if (!neg && v < 0) throw WabiProtocolException('integer out of range for s64');
    return v;
  }

  int readIntRange(int min, int max) {
    final v = readInt();
    if (v < min || v > max) throw WabiProtocolException('integer $v out of range');
    return v;
  }

  double readFloat() {
    final m = _next();
    _need(m == 0xca ? 4 : 8);
    final v = switch (m) {
      0xca => _data.getFloat32(_pos),
      0xcb => _data.getFloat64(_pos),
      _ => throw _unexpected('float', m),
    };
    _pos += m == 0xca ? 4 : 8;
    return v;
  }

  int _strLen() {
    final m = _next();
    if (m >= 0xa0 && m <= 0xbf) return m & 0x1f;
    return switch (m) {
      0xd9 => _be(1),
      0xda => _be(2),
      0xdb => _be(4),
      _ => throw _unexpected('string', m),
    };
  }

  /// Invalid UTF-8 is rejected, never replaced.
  String readString() {
    final bytes = _take(_strLen());
    try {
      return utf8.decode(bytes);
    } on FormatException {
      throw WabiProtocolException('string is not valid UTF-8');
    }
  }

  /// Returns a copy (safe to keep after the reader is gone).
  Uint8List readBinary() {
    final m = _next();
    final n = switch (m) {
      0xc4 => _be(1),
      0xc5 => _be(2),
      0xc6 => _be(4),
      _ => throw _unexpected('bin', m),
    };
    return Uint8List.fromList(_take(n));
  }

  int readArrayHeader() {
    final m = _next();
    final n = (m >= 0x90 && m <= 0x9f)
        ? m & 0x0f
        : switch (m) { 0xdc => _be(2), 0xdd => _be(4), _ => throw _unexpected('array', m) };
    if (n > remaining) throw WabiProtocolException('array length exceeds payload');
    return n;
  }

  int readMapHeader() {
    final m = _next();
    final n = (m >= 0x80 && m <= 0x8f)
        ? m & 0x0f
        : switch (m) { 0xde => _be(2), 0xdf => _be(4), _ => throw _unexpected('map', m) };
    if (n * 2 > remaining) throw WabiProtocolException('map length exceeds payload');
    return n;
  }

  void readTupleHeader(int expected) {
    final n = readArrayHeader();
    if (n != expected) throw WabiProtocolException('expected tuple of $expected, found $n');
  }

  List<T> readList<T>(T Function(MsgPackReader) item) {
    final n = readArrayHeader();
    return List<T>.unmodifiable([for (var i = 0; i < n; i++) item(this)]);
  }

  T? readOptional<T extends Object>(T Function(MsgPackReader) item) => tryReadNil() ? null : item(this);

  /// `result<T, E>` is encoded as `[tag, value]`: 0 = ok, 1 = err.
  int readResultTag() {
    final n = readArrayHeader();
    if (n != 2) throw WabiProtocolException('result must have 2 elements, found $n');
    final tag = readUint();
    if (tag > 1) throw WabiProtocolException('invalid result tag $tag');
    return tag;
  }

  void skip([int depth = 0]) {
    if (depth > _maxDepth) throw WabiProtocolException('payload nested too deeply');
    final m = _peek();
    if (m <= 0x7f || m >= 0xe0 || (m >= 0xcc && m <= 0xd3)) {
      _integer();
    } else if (m == 0xc0 || m == 0xc2 || m == 0xc3) {
      _pos++;
    } else if (m == 0xca || m == 0xcb) {
      readFloat();
    } else if ((m >= 0xa0 && m <= 0xbf) || (m >= 0xd9 && m <= 0xdb)) {
      _take(_strLen());
    } else if (m >= 0xc4 && m <= 0xc6) {
      readBinary();
    } else if ((m >= 0x90 && m <= 0x9f) || m == 0xdc || m == 0xdd) {
      for (var i = readArrayHeader(); i > 0; i--) {
        skip(depth + 1);
      }
    } else if ((m >= 0x80 && m <= 0x8f) || m == 0xde || m == 0xdf) {
      for (var i = readMapHeader() * 2; i > 0; i--) {
        skip(depth + 1);
      }
    } else if (m >= 0xd4 && m <= 0xd8) {
      _pos++;
      _take(1 + (1 << (m - 0xd4)));
    } else if (m >= 0xc7 && m <= 0xc9) {
      _pos++;
      _take(1 + _be(const {0xc7: 1, 0xc8: 2, 0xc9: 4}[m]!));
    } else {
      throw _unexpected('value', m);
    }
  }
}
