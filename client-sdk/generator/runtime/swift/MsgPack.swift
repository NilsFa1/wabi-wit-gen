// Hand-written, generic: payload encoding v1 (MessagePack subset, ABI.md §5).

public final class MsgPackWriter {
    public private(set) var bytes: [UInt8] = []

    public init() { bytes.reserveCapacity(64) }

    private func be<T: FixedWidthInteger>(_ v: T) {
        withUnsafeBytes(of: v.bigEndian) { bytes.append(contentsOf: $0) }
    }

    public func writeNil() { bytes.append(0xc0) }
    public func writeBool(_ v: Bool) { bytes.append(v ? 0xc3 : 0xc2) }

    public func writeUInt(_ v: UInt64) {
        switch v {
        case 0..<0x80: bytes.append(UInt8(v))
        case 0...0xFF: bytes.append(0xcc); bytes.append(UInt8(v))
        case 0...0xFFFF: bytes.append(0xcd); be(UInt16(v))
        case 0...0xFFFF_FFFF: bytes.append(0xce); be(UInt32(v))
        default: bytes.append(0xcf); be(v)
        }
    }

    public func writeInt(_ v: Int64) {
        if v >= 0 { writeUInt(UInt64(v)) }
        else if v >= -32 { bytes.append(UInt8(bitPattern: Int8(v))) }
        else if v >= Int64(Int8.min) { bytes.append(0xd0); be(Int8(v)) }
        else if v >= Int64(Int16.min) { bytes.append(0xd1); be(Int16(v)) }
        else if v >= Int64(Int32.min) { bytes.append(0xd2); be(Int32(v)) }
        else { bytes.append(0xd3); be(v) }
    }

    public func writeFloat32(_ v: Float) { bytes.append(0xca); be(v.bitPattern) }
    public func writeFloat64(_ v: Double) { bytes.append(0xcb); be(v.bitPattern) }

    private func header(_ n: Int, fix: (base: UInt8, max: Int)?, m8: UInt8?, m16: UInt8, m32: UInt8) {
        if let fix, n <= fix.max { bytes.append(fix.base | UInt8(n)) }
        else if let m8, n <= 0xFF { bytes.append(m8); bytes.append(UInt8(n)) }
        else if n <= 0xFFFF { bytes.append(m16); be(UInt16(n)) }
        else { bytes.append(m32); be(UInt32(n)) }
    }

    /// Swift strings are always valid Unicode; they travel as UTF-8.
    public func writeString(_ s: String) {
        let utf8 = Array(s.utf8)
        header(utf8.count, fix: (0xa0, 31), m8: 0xd9, m16: 0xda, m32: 0xdb)
        bytes.append(contentsOf: utf8)
    }

    public func writeBinary(_ b: [UInt8]) {
        header(b.count, fix: nil, m8: 0xc4, m16: 0xc5, m32: 0xc6)
        bytes.append(contentsOf: b)
    }

    public func writeArrayHeader(_ n: Int) { header(n, fix: (0x90, 15), m8: nil, m16: 0xdc, m32: 0xdd) }
    public func writeMapHeader(_ n: Int) { header(n, fix: (0x80, 15), m8: nil, m16: 0xde, m32: 0xdf) }
}

public final class MsgPackReader {
    private let buf: [UInt8]
    private var pos = 0
    private static let maxDepth = 64

    public init(_ buf: [UInt8]) { self.buf = buf }

    public var remaining: Int { buf.count - pos }

    private func peek() throws -> UInt8 {
        guard pos < buf.count else { throw WabiRuntimeError.protocolError("unexpected end of payload") }
        return buf[pos]
    }

    private func next() throws -> UInt8 {
        let b = try peek()
        pos += 1
        return b
    }

    private func be(_ n: Int) throws -> UInt64 {
        guard remaining >= n else { throw WabiRuntimeError.protocolError("unexpected end of payload") }
        var v: UInt64 = 0
        for _ in 0..<n { v = (v << 8) | UInt64(buf[pos]); pos += 1 }
        return v
    }

    private func take(_ n: UInt64) throws -> ArraySlice<UInt8> {
        guard n <= UInt64(remaining) else { throw WabiRuntimeError.protocolError("length exceeds payload") }
        let s = buf[pos..<(pos + Int(n))]
        pos += Int(n)
        return s
    }

    private func unexpected(_ what: String, _ m: UInt8) -> WabiRuntimeError {
        .protocolError("expected \(what), found marker 0x\(String(m, radix: 16))")
    }

    public func tryReadNil() throws -> Bool {
        guard try peek() == 0xc0 else { return false }
        pos += 1
        return true
    }

    public func readNil() throws {
        let m = try next()
        guard m == 0xc0 else { throw unexpected("nil", m) }
    }

    public func readBool() throws -> Bool {
        switch try next() {
        case 0xc2: return false
        case 0xc3: return true
        case let m: throw unexpected("bool", m)
        }
    }

    /// Any MessagePack integer, as (isNegative, magnitudeBits/value).
    private func readInteger() throws -> (neg: Bool, u: UInt64, s: Int64) {
        let m = try next()
        switch m {
        case 0x00...0x7f: return (false, UInt64(m), Int64(m))
        case 0xe0...0xff: return (true, 0, Int64(Int8(bitPattern: m)))
        case 0xcc: let v = try be(1); return (false, v, Int64(v))
        case 0xcd: let v = try be(2); return (false, v, Int64(v))
        case 0xce: let v = try be(4); return (false, v, Int64(v))
        case 0xcf: let v = try be(8); return (false, v, Int64(bitPattern: v))
        case 0xd0: let v = Int64(Int8(truncatingIfNeeded: try be(1))); return (v < 0, v < 0 ? 0 : UInt64(v), v)
        case 0xd1: let v = Int64(Int16(truncatingIfNeeded: try be(2))); return (v < 0, v < 0 ? 0 : UInt64(v), v)
        case 0xd2: let v = Int64(Int32(truncatingIfNeeded: try be(4))); return (v < 0, v < 0 ? 0 : UInt64(v), v)
        case 0xd3: let v = Int64(bitPattern: try be(8)); return (v < 0, v < 0 ? 0 : UInt64(v), v)
        default: throw unexpected("integer", m)
        }
    }

    public func readUInt<T: FixedWidthInteger & UnsignedInteger>(_: T.Type) throws -> T {
        let (neg, u, _) = try readInteger()
        guard !neg, let v = T(exactly: u) else { throw WabiRuntimeError.protocolError("integer out of range for \(T.self)") }
        return v
    }

    public func readInt<T: FixedWidthInteger & SignedInteger>(_: T.Type) throws -> T {
        let (neg, u, s) = try readInteger()
        if !neg && u > UInt64(Int64.max) { throw WabiRuntimeError.protocolError("integer out of range for \(T.self)") }
        guard let v = T(exactly: s) else { throw WabiRuntimeError.protocolError("integer out of range for \(T.self)") }
        return v
    }

    public func readScalar() throws -> Unicode.Scalar {
        let v = try readUInt(UInt32.self)
        guard let s = Unicode.Scalar(v) else { throw WabiRuntimeError.protocolError("\(v) is not a unicode scalar value") }
        return s
    }

    public func readFloat64() throws -> Double {
        switch try next() {
        case 0xca: return Double(Float(bitPattern: UInt32(try be(4))))
        case 0xcb: return Double(bitPattern: try be(8))
        case let m: throw unexpected("float", m)
        }
    }

    private func strLen() throws -> UInt64 {
        let m = try next()
        switch m {
        case 0xa0...0xbf: return UInt64(m & 0x1f)
        case 0xd9: return try be(1)
        case 0xda: return try be(2)
        case 0xdb: return try be(4)
        default: throw unexpected("string", m)
        }
    }

    /// Invalid UTF-8 is rejected, never repaired.
    public func readString() throws -> String {
        let n = try strLen()
        let bytes = try take(n)
        // String(decoding:) repairs invalid input with U+FFFD; a repaired string no
        // longer round-trips, which is how invalid UTF-8 is detected (Swift 5.9+).
        let s = String(decoding: bytes, as: UTF8.self)
        guard s.utf8.elementsEqual(bytes) else {
            throw WabiRuntimeError.protocolError("string is not valid UTF-8")
        }
        return s
    }

    public func readBinary() throws -> [UInt8] {
        let m = try next()
        let n: UInt64
        switch m {
        case 0xc4: n = try be(1)
        case 0xc5: n = try be(2)
        case 0xc6: n = try be(4)
        default: throw unexpected("bin", m)
        }
        return Array(try take(n))
    }

    public func readArrayHeader() throws -> Int {
        let m = try next()
        let n: UInt64
        switch m {
        case 0x90...0x9f: n = UInt64(m & 0x0f)
        case 0xdc: n = try be(2)
        case 0xdd: n = try be(4)
        default: throw unexpected("array", m)
        }
        guard n <= UInt64(remaining) else { throw WabiRuntimeError.protocolError("array length exceeds payload") }
        return Int(n)
    }

    public func readMapHeader() throws -> Int {
        let m = try next()
        let n: UInt64
        switch m {
        case 0x80...0x8f: n = UInt64(m & 0x0f)
        case 0xde: n = try be(2)
        case 0xdf: n = try be(4)
        default: throw unexpected("map", m)
        }
        guard n * 2 <= UInt64(remaining) else { throw WabiRuntimeError.protocolError("map length exceeds payload") }
        return Int(n)
    }

    public func readTupleHeader(_ expected: Int) throws {
        let n = try readArrayHeader()
        guard n == expected else { throw WabiRuntimeError.protocolError("expected tuple of \(expected), found \(n)") }
    }

    public func readList<T>(_ item: (MsgPackReader) throws -> T) throws -> [T] {
        let n = try readArrayHeader()
        var out: [T] = []
        out.reserveCapacity(n)
        for _ in 0..<n { out.append(try item(self)) }
        return out
    }

    public func readOptional<T>(_ item: (MsgPackReader) throws -> T) throws -> T? {
        if try tryReadNil() { return nil }
        return try item(self)
    }

    /// result<T, E> is encoded as [tag, value]: 0 = ok, 1 = err.
    public func readResultTag() throws -> UInt64 {
        let n = try readArrayHeader()
        guard n == 2 else { throw WabiRuntimeError.protocolError("result must have 2 elements, found \(n)") }
        let tag = try readUInt(UInt64.self)
        guard tag <= 1 else { throw WabiRuntimeError.protocolError("invalid result tag \(tag)") }
        return tag
    }

    public func skip() throws { try skip(depth: 0) }

    private func skip(depth: Int) throws {
        guard depth <= MsgPackReader.maxDepth else { throw WabiRuntimeError.protocolError("payload nested too deeply") }
        let m = try peek()
        switch m {
        case 0x00...0x7f, 0xe0...0xff, 0xcc...0xd3: _ = try readInteger()
        case 0xc0, 0xc2, 0xc3: pos += 1
        case 0xca, 0xcb: _ = try readFloat64()
        case 0xa0...0xbf, 0xd9...0xdb: let n = try strLen(); _ = try take(n)
        case 0xc4...0xc6: _ = try readBinary()
        case 0x90...0x9f, 0xdc, 0xdd:
            let n = try readArrayHeader()
            for _ in 0..<n { try skip(depth: depth + 1) }
        case 0x80...0x8f, 0xde, 0xdf:
            let n = try readMapHeader()
            for _ in 0..<(n * 2) { try skip(depth: depth + 1) }
        case 0xd4...0xd8:
            pos += 1
            _ = try take(1 + (UInt64(1) << UInt64(m - 0xd4)))
        case 0xc7, 0xc8, 0xc9:
            pos += 1
            let n = try be(m == 0xc7 ? 1 : m == 0xc8 ? 2 : 4)
            _ = try take(1 + n)
        default: throw unexpected("value", m)
        }
    }
}
