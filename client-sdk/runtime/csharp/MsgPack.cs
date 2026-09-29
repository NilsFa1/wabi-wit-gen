// Hand-written, generic: payload encoding v1 (MessagePack subset, ABI.md §5).
// Knows nothing about customers, clients or any other contract type.
#nullable enable
using System;
using System.Buffers.Binary;
using System.Collections.Generic;
using System.Text;

namespace Acme.Client.Runtime;

public sealed class MsgPackWriter
{
    private byte[] _buf = new byte[64];
    private int _len;

    private Span<byte> Grow(int n)
    {
        if (_len + n > _buf.Length)
            Array.Resize(ref _buf, Math.Max(_buf.Length * 2, _len + n));
        var s = _buf.AsSpan(_len, n);
        _len += n;
        return s;
    }

    private void Byte(byte b) => Grow(1)[0] = b;

    public byte[] ToArray() => _buf.AsSpan(0, _len).ToArray();

    public void WriteNil() => Byte(0xc0);
    public void WriteBool(bool v) => Byte(v ? (byte)0xc3 : (byte)0xc2);

    public void WriteUInt(ulong v)
    {
        if (v < 0x80) Byte((byte)v);
        else if (v <= byte.MaxValue) { var s = Grow(2); s[0] = 0xcc; s[1] = (byte)v; }
        else if (v <= ushort.MaxValue) { Byte(0xcd); BinaryPrimitives.WriteUInt16BigEndian(Grow(2), (ushort)v); }
        else if (v <= uint.MaxValue) { Byte(0xce); BinaryPrimitives.WriteUInt32BigEndian(Grow(4), (uint)v); }
        else { Byte(0xcf); BinaryPrimitives.WriteUInt64BigEndian(Grow(8), v); }
    }

    public void WriteInt(long v)
    {
        if (v >= 0) WriteUInt((ulong)v);
        else if (v >= -32) Byte((byte)(sbyte)v);
        else if (v >= sbyte.MinValue) { var s = Grow(2); s[0] = 0xd0; s[1] = (byte)(sbyte)v; }
        else if (v >= short.MinValue) { Byte(0xd1); BinaryPrimitives.WriteInt16BigEndian(Grow(2), (short)v); }
        else if (v >= int.MinValue) { Byte(0xd2); BinaryPrimitives.WriteInt32BigEndian(Grow(4), (int)v); }
        else { Byte(0xd3); BinaryPrimitives.WriteInt64BigEndian(Grow(8), v); }
    }

    public void WriteFloat32(float v) { Byte(0xca); BinaryPrimitives.WriteSingleBigEndian(Grow(4), v); }
    public void WriteFloat64(double v) { Byte(0xcb); BinaryPrimitives.WriteDoubleBigEndian(Grow(8), v); }

    private void Header(int len, byte fixBase, int fixMax, byte? m8, byte m16, byte m32)
    {
        if (fixMax > 0 && len <= fixMax) Byte((byte)(fixBase | len));
        else if (m8 is byte b8 && len <= byte.MaxValue) { var s = Grow(2); s[0] = b8; s[1] = (byte)len; }
        else if (len <= ushort.MaxValue) { Byte(m16); BinaryPrimitives.WriteUInt16BigEndian(Grow(2), (ushort)len); }
        else { Byte(m32); BinaryPrimitives.WriteUInt32BigEndian(Grow(4), (uint)len); }
    }

    /// <summary>Strings always travel as UTF-8. Lone surrogates cannot be encoded and throw.</summary>
    public void WriteString(string s)
    {
        ArgumentNullException.ThrowIfNull(s);
        int n = StrictUtf8.GetByteCount(s);
        Header(n, 0xa0, 31, 0xd9, 0xda, 0xdb);
        StrictUtf8.GetBytes(s, Grow(n));
    }

    public void WriteBinary(ReadOnlySpan<byte> b)
    {
        Header(b.Length, 0, 0, 0xc4, 0xc5, 0xc6);
        b.CopyTo(Grow(b.Length));
    }

    public void WriteArrayHeader(int n) => Header(n, 0x90, 15, null, 0xdc, 0xdd);
    public void WriteMapHeader(int n) => Header(n, 0x80, 15, null, 0xde, 0xdf);

    internal static readonly UTF8Encoding StrictUtf8 = new(false, throwOnInvalidBytes: true);
}

public sealed class MsgPackReader
{
    private readonly byte[] _buf;
    private int _pos;
    private const int MaxDepth = 64;

    public MsgPackReader(byte[] buf) => _buf = buf;

    public int Remaining => _buf.Length - _pos;

    private byte Peek() => _pos < _buf.Length ? _buf[_pos] : throw Eof();
    private byte Next() => _pos < _buf.Length ? _buf[_pos++] : throw Eof();
    private static AcmeProtocolException Eof() => new("unexpected end of payload");

    private ReadOnlySpan<byte> Take(long n)
    {
        if (n < 0 || n > Remaining) throw new AcmeProtocolException("length exceeds payload");
        var s = _buf.AsSpan(_pos, (int)n);
        _pos += (int)n;
        return s;
    }

    private static AcmeProtocolException Unexpected(string what, byte m) => new($"expected {what}, found marker 0x{m:x2}");

    public bool TryReadNil()
    {
        if (Peek() != 0xc0) return false;
        _pos++;
        return true;
    }

    public void ReadNil() { var m = Next(); if (m != 0xc0) throw Unexpected("nil", m); }

    public bool ReadBool() => Next() switch
    {
        0xc2 => false,
        0xc3 => true,
        var m => throw Unexpected("bool", m),
    };

    /// <summary>Reads any integer encoding; returns (negative, magnitude-or-value).</summary>
    private (bool Neg, ulong U, long S) ReadInteger()
    {
        var m = Next();
        switch (m)
        {
            case <= 0x7f: return (false, m, m);
            case >= 0xe0: return (true, 0, (sbyte)m);
            case 0xcc: { var v = Take(1)[0]; return (false, v, v); }
            case 0xcd: { var v = BinaryPrimitives.ReadUInt16BigEndian(Take(2)); return (false, v, v); }
            case 0xce: { var v = BinaryPrimitives.ReadUInt32BigEndian(Take(4)); return (false, v, v); }
            case 0xcf: { var v = BinaryPrimitives.ReadUInt64BigEndian(Take(8)); return (false, v, unchecked((long)v)); }
            case 0xd0: { var v = (sbyte)Take(1)[0]; return (v < 0, (ulong)Math.Max((long)v, 0), v); }
            case 0xd1: { var v = BinaryPrimitives.ReadInt16BigEndian(Take(2)); return (v < 0, (ulong)Math.Max((long)v, 0), v); }
            case 0xd2: { var v = BinaryPrimitives.ReadInt32BigEndian(Take(4)); return (v < 0, (ulong)Math.Max((long)v, 0), v); }
            case 0xd3: { var v = BinaryPrimitives.ReadInt64BigEndian(Take(8)); return (v < 0, (ulong)Math.Max(v, 0), v); }
            default: throw Unexpected("integer", m);
        }
    }

    public ulong ReadUInt64()
    {
        var (neg, u, _) = ReadInteger();
        if (neg) throw new AcmeProtocolException("negative value for unsigned integer");
        return u;
    }

    public long ReadInt64()
    {
        var (neg, u, s) = ReadInteger();
        if (!neg && u > long.MaxValue) throw new AcmeProtocolException("integer out of range for s64");
        return s;
    }

    public float ReadFloat32() => (float)ReadFloat64();

    public double ReadFloat64() => Next() switch
    {
        0xca => BinaryPrimitives.ReadSingleBigEndian(Take(4)),
        0xcb => BinaryPrimitives.ReadDoubleBigEndian(Take(8)),
        var m => throw Unexpected("float", m),
    };

    private long StrLen()
    {
        var m = Next();
        return m switch
        {
            >= 0xa0 and <= 0xbf => m & 0x1f,
            0xd9 => Take(1)[0],
            0xda => BinaryPrimitives.ReadUInt16BigEndian(Take(2)),
            0xdb => BinaryPrimitives.ReadUInt32BigEndian(Take(4)),
            _ => throw Unexpected("string", m),
        };
    }

    /// <summary>Invalid UTF-8 is rejected (never silently replaced).</summary>
    public string ReadString()
    {
        var bytes = Take(StrLen());
        try { return MsgPackWriter.StrictUtf8.GetString(bytes); }
        catch (DecoderFallbackException) { throw new AcmeProtocolException("string is not valid UTF-8"); }
    }

    public byte[] ReadBinary()
    {
        var m = Next();
        long n = m switch
        {
            0xc4 => Take(1)[0],
            0xc5 => BinaryPrimitives.ReadUInt16BigEndian(Take(2)),
            0xc6 => BinaryPrimitives.ReadUInt32BigEndian(Take(4)),
            _ => throw Unexpected("bin", m),
        };
        return Take(n).ToArray();
    }

    public int ReadArrayHeader()
    {
        var m = Next();
        long n = m switch
        {
            >= 0x90 and <= 0x9f => m & 0x0f,
            0xdc => BinaryPrimitives.ReadUInt16BigEndian(Take(2)),
            0xdd => BinaryPrimitives.ReadUInt32BigEndian(Take(4)),
            _ => throw Unexpected("array", m),
        };
        if (n > Remaining) throw new AcmeProtocolException("array length exceeds payload");
        return (int)n;
    }

    public void ReadArrayHeader(int expected)
    {
        var n = ReadArrayHeader();
        if (n != expected) throw new AcmeProtocolException($"expected tuple of {expected}, found {n}");
    }

    public int ReadMapHeader()
    {
        var m = Next();
        long n = m switch
        {
            >= 0x80 and <= 0x8f => m & 0x0f,
            0xde => BinaryPrimitives.ReadUInt16BigEndian(Take(2)),
            0xdf => BinaryPrimitives.ReadUInt32BigEndian(Take(4)),
            _ => throw Unexpected("map", m),
        };
        if (n * 2 > Remaining) throw new AcmeProtocolException("map length exceeds payload");
        return (int)n;
    }

    public IReadOnlyList<T> ReadList<T>(Func<MsgPackReader, T> item)
    {
        var n = ReadArrayHeader();
        var list = new List<T>(n);
        for (int i = 0; i < n; i++) list.Add(item(this));
        return list;
    }

    /// <summary>result&lt;T, E&gt; is encoded as [tag, value]: 0 = ok, 1 = err.</summary>
    public ulong ReadResultTag()
    {
        var n = ReadArrayHeader();
        if (n != 2) throw new AcmeProtocolException($"result must have 2 elements, found {n}");
        var tag = ReadUInt64();
        if (tag > 1) throw new AcmeProtocolException($"invalid result tag {tag}");
        return tag;
    }

    public void Skip() => Skip(0);

    private void Skip(int depth)
    {
        if (depth > MaxDepth) throw new AcmeProtocolException("payload nested too deeply");
        var m = Peek();
        switch (m)
        {
            case <= 0x7f or >= 0xe0 or (>= 0xcc and <= 0xd3): ReadInteger(); break;
            case 0xc0 or 0xc2 or 0xc3: _pos++; break;
            case 0xca or 0xcb: ReadFloat64(); break;
            case (>= 0xa0 and <= 0xbf) or (>= 0xd9 and <= 0xdb): Take(StrLen()); break;
            case >= 0xc4 and <= 0xc6: ReadBinary(); break;
            case (>= 0x90 and <= 0x9f) or 0xdc or 0xdd:
                for (int i = ReadArrayHeader(); i > 0; i--) Skip(depth + 1);
                break;
            case (>= 0x80 and <= 0x8f) or 0xde or 0xdf:
                for (int i = ReadMapHeader() * 2; i > 0; i--) Skip(depth + 1);
                break;
            case >= 0xd4 and <= 0xd8:
                _pos++; Take(1 + (1 << (m - 0xd4))); break;
            case 0xc7: _pos++; Take(1 + Take(1)[0]); break;
            case 0xc8: _pos++; Take(1 + BinaryPrimitives.ReadUInt16BigEndian(Take(2))); break;
            case 0xc9: _pos++; Take(1 + (long)BinaryPrimitives.ReadUInt32BigEndian(Take(4))); break;
            default: throw Unexpected("value", m);
        }
    }
}
