// Hand-written, generic: payload encoding v1 (MessagePack subset, ABI.md §5).
package com.wabi.client.runtime;

import java.nio.ByteBuffer;
import java.nio.charset.CharacterCodingException;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Optional;
import java.util.function.Function;

public final class MsgPackReader {
    private static final int MAX_DEPTH = 64;
    private final byte[] buf;
    private int pos;

    public MsgPackReader(byte[] buf) {
        this.buf = buf;
    }

    public int remaining() {
        return buf.length - pos;
    }

    private int peek() {
        if (pos >= buf.length) throw eof();
        return buf[pos] & 0xFF;
    }

    private int next() {
        if (pos >= buf.length) throw eof();
        return buf[pos++] & 0xFF;
    }

    private static WabiProtocolException eof() {
        return new WabiProtocolException("unexpected end of payload");
    }

    private static WabiProtocolException unexpected(String what, int m) {
        return new WabiProtocolException("expected " + what + ", found marker 0x" + Integer.toHexString(m));
    }

    private long be(int bytes) {
        if (remaining() < bytes) throw eof();
        long v = 0;
        for (int i = 0; i < bytes; i++) v = (v << 8) | (buf[pos++] & 0xFF);
        return v;
    }

    private int take(long n) {
        if (n < 0 || n > remaining()) throw new WabiProtocolException("length exceeds payload");
        int start = pos;
        pos += (int) n;
        return start;
    }

    public boolean tryReadNil() {
        if (peek() != 0xc0) return false;
        pos++;
        return true;
    }

    public void readNil() {
        int m = next();
        if (m != 0xc0) throw unexpected("nil", m);
    }

    public boolean readBool() {
        int m = next();
        if (m == 0xc2) return false;
        if (m == 0xc3) return true;
        throw unexpected("bool", m);
    }

    /** Returns {negativeFlag, value}: value is raw unsigned bits when non-negative. */
    private long readInteger(boolean[] negative) {
        int m = next();
        negative[0] = false;
        if (m <= 0x7f) return m;
        if (m >= 0xe0) { negative[0] = true; return (byte) m; }
        switch (m) {
            case 0xcc: return be(1);
            case 0xcd: return be(2);
            case 0xce: return be(4);
            case 0xcf: return be(8);
            case 0xd0: { long v = (byte) be(1); negative[0] = v < 0; return v; }
            case 0xd1: { long v = (short) be(2); negative[0] = v < 0; return v; }
            case 0xd2: { long v = (int) be(4); negative[0] = v < 0; return v; }
            case 0xd3: { long v = be(8); negative[0] = v < 0; return v; }
            default: throw unexpected("integer", m);
        }
    }

    /** Unsigned 64-bit value as raw bits (use Long.toUnsignedString to print). */
    public long readUInt64() {
        boolean[] neg = new boolean[1];
        long v = readInteger(neg);
        if (neg[0]) throw new WabiProtocolException("negative value for unsigned integer");
        return v;
    }

    public long readUIntMax(long max) {
        long v = readUInt64();
        if (Long.compareUnsigned(v, max) > 0) throw new WabiProtocolException("integer " + Long.toUnsignedString(v) + " out of range");
        return v;
    }

    public long readInt64() {
        boolean[] neg = new boolean[1];
        long v = readInteger(neg);
        if (!neg[0] && v < 0) throw new WabiProtocolException("integer out of range for s64");
        return v;
    }

    public long readIntRange(long min, long max) {
        long v = readInt64();
        if (v < min || v > max) throw new WabiProtocolException("integer " + v + " out of range");
        return v;
    }

    public float readFloat32() {
        return (float) readFloat64();
    }

    public double readFloat64() {
        int m = next();
        if (m == 0xca) return Float.intBitsToFloat((int) be(4));
        if (m == 0xcb) return Double.longBitsToDouble(be(8));
        throw unexpected("float", m);
    }

    private long strLen() {
        int m = next();
        if (m >= 0xa0 && m <= 0xbf) return m & 0x1f;
        if (m == 0xd9) return be(1);
        if (m == 0xda) return be(2);
        if (m == 0xdb) return be(4);
        throw unexpected("string", m);
    }

    /** Invalid UTF-8 is rejected, never replaced. */
    public String readString() {
        long n = strLen();
        int start = take(n);
        try {
            return StandardCharsets.UTF_8.newDecoder()
                    .onMalformedInput(CodingErrorAction.REPORT)
                    .onUnmappableCharacter(CodingErrorAction.REPORT)
                    .decode(ByteBuffer.wrap(buf, start, (int) n)).toString();
        } catch (CharacterCodingException e) {
            throw new WabiProtocolException("string is not valid UTF-8");
        }
    }

    public byte[] readBinary() {
        int m = next();
        long n;
        if (m == 0xc4) n = be(1);
        else if (m == 0xc5) n = be(2);
        else if (m == 0xc6) n = be(4);
        else throw unexpected("bin", m);
        int start = take(n);
        byte[] out = new byte[(int) n];
        System.arraycopy(buf, start, out, 0, (int) n);
        return out;
    }

    public int readArrayHeader() {
        int m = next();
        long n;
        if (m >= 0x90 && m <= 0x9f) n = m & 0x0f;
        else if (m == 0xdc) n = be(2);
        else if (m == 0xdd) n = be(4);
        else throw unexpected("array", m);
        if (n > remaining()) throw new WabiProtocolException("array length exceeds payload");
        return (int) n;
    }

    public int readMapHeader() {
        int m = next();
        long n;
        if (m >= 0x80 && m <= 0x8f) n = m & 0x0f;
        else if (m == 0xde) n = be(2);
        else if (m == 0xdf) n = be(4);
        else throw unexpected("map", m);
        if (n * 2 > remaining()) throw new WabiProtocolException("map length exceeds payload");
        return (int) n;
    }

    public <T> List<T> readList(Function<MsgPackReader, T> item) {
        int n = readArrayHeader();
        var list = new ArrayList<T>(n);
        for (int i = 0; i < n; i++) list.add(item.apply(this));
        return Collections.unmodifiableList(list);
    }

    public <T> Optional<T> readOptional(Function<MsgPackReader, T> item) {
        return tryReadNil() ? Optional.empty() : Optional.of(item.apply(this));
    }

    /** result&lt;T, E&gt; is encoded as [tag, value]: 0 = ok, 1 = err. */
    public int readResultTag() {
        int n = readArrayHeader();
        if (n != 2) throw new WabiProtocolException("result must have 2 elements, found " + n);
        long tag = readUInt64();
        if (tag > 1) throw new WabiProtocolException("invalid result tag " + tag);
        return (int) tag;
    }

    public void skip() {
        skip(0);
    }

    private void skip(int depth) {
        if (depth > MAX_DEPTH) throw new WabiProtocolException("payload nested too deeply");
        int m = peek();
        if (m <= 0x7f || m >= 0xe0 || (m >= 0xcc && m <= 0xd3)) readInteger(new boolean[1]);
        else if (m == 0xc0 || m == 0xc2 || m == 0xc3) pos++;
        else if (m == 0xca || m == 0xcb) readFloat64();
        else if ((m >= 0xa0 && m <= 0xbf) || (m >= 0xd9 && m <= 0xdb)) take(strLen());
        else if (m >= 0xc4 && m <= 0xc6) readBinary();
        else if ((m >= 0x90 && m <= 0x9f) || m == 0xdc || m == 0xdd) {
            for (int i = readArrayHeader(); i > 0; i--) skip(depth + 1);
        } else if ((m >= 0x80 && m <= 0x8f) || m == 0xde || m == 0xdf) {
            for (int i = readMapHeader() * 2; i > 0; i--) skip(depth + 1);
        } else if (m >= 0xd4 && m <= 0xd8) {
            pos++;
            take(1 + (1L << (m - 0xd4)));
        } else if (m == 0xc7) { pos++; take(1 + be(1)); }
        else if (m == 0xc8) { pos++; take(1 + be(2)); }
        else if (m == 0xc9) { pos++; take(1 + be(4)); }
        else throw unexpected("value", m);
    }
}
