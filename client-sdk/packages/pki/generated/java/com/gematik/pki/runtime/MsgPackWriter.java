// Copied by wabi-wit-gen from generator/runtime — DO NOT EDIT; change the source there.
// Hand-written, generic: payload encoding v1 (MessagePack subset, ABI.md §5).
package com.gematik.pki.runtime;

import java.nio.ByteBuffer;
import java.nio.CharBuffer;
import java.nio.charset.CharacterCodingException;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;

public final class MsgPackWriter {
    private byte[] buf = new byte[64];
    private int len;

    private void ensure(int n) {
        if (len + n > buf.length) buf = Arrays.copyOf(buf, Math.max(buf.length * 2, len + n));
    }

    private void b(int v) {
        ensure(1);
        buf[len++] = (byte) v;
    }

    private void be(long v, int bytes) {
        ensure(bytes);
        for (int i = bytes - 1; i >= 0; i--) buf[len++] = (byte) (v >>> (8 * i));
    }

    public byte[] toByteArray() {
        return Arrays.copyOf(buf, len);
    }

    public void writeNil() {
        b(0xc0);
    }

    public void writeBool(boolean v) {
        b(v ? 0xc3 : 0xc2);
    }

    /** Non-negative value 0..Long.MAX_VALUE. */
    public void writeUInt(long v) {
        if (v < 0) throw new IllegalArgumentException("negative value for unsigned integer");
        writeUInt64(v);
    }

    /** Full 64-bit unsigned range: {@code v} is interpreted as unsigned bits. */
    public void writeUInt64(long v) {
        if (Long.compareUnsigned(v, 0x80) < 0) b((int) v);
        else if (Long.compareUnsigned(v, 0xFF) <= 0) { b(0xcc); be(v, 1); }
        else if (Long.compareUnsigned(v, 0xFFFF) <= 0) { b(0xcd); be(v, 2); }
        else if (Long.compareUnsigned(v, 0xFFFFFFFFL) <= 0) { b(0xce); be(v, 4); }
        else { b(0xcf); be(v, 8); }
    }

    public void writeInt(long v) {
        if (v >= 0) writeUInt64(v);
        else if (v >= -32) b((int) v);
        else if (v >= Byte.MIN_VALUE) { b(0xd0); be(v, 1); }
        else if (v >= Short.MIN_VALUE) { b(0xd1); be(v, 2); }
        else if (v >= Integer.MIN_VALUE) { b(0xd2); be(v, 4); }
        else { b(0xd3); be(v, 8); }
    }

    public void writeFloat32(float v) {
        b(0xca);
        be(Float.floatToIntBits(v), 4);
    }

    public void writeFloat64(double v) {
        b(0xcb);
        be(Double.doubleToLongBits(v), 8);
    }

    private void header(int n, int fixBase, int fixMax, int m8, int m16, int m32) {
        if (fixMax > 0 && n <= fixMax) b(fixBase | n);
        else if (m8 != 0 && n <= 0xFF) { b(m8); be(n, 1); }
        else if (n <= 0xFFFF) { b(m16); be(n, 2); }
        else { b(m32); be(n, 4); }
    }

    /** UTF-8; unpaired surrogates are rejected with IllegalArgumentException. */
    public void writeString(String s) {
        ByteBuffer bytes;
        try {
            bytes = StandardCharsets.UTF_8.newEncoder()
                    .onMalformedInput(CodingErrorAction.REPORT)
                    .onUnmappableCharacter(CodingErrorAction.REPORT)
                    .encode(CharBuffer.wrap(s));
        } catch (CharacterCodingException e) {
            throw new IllegalArgumentException("string is not valid Unicode (unpaired surrogate?)", e);
        }
        int n = bytes.remaining();
        header(n, 0xa0, 31, 0xd9, 0xda, 0xdb);
        ensure(n);
        bytes.get(buf, len, n);
        len += n;
    }

    public void writeBinary(byte[] v) {
        header(v.length, 0, 0, 0xc4, 0xc5, 0xc6);
        ensure(v.length);
        System.arraycopy(v, 0, buf, len, v.length);
        len += v.length;
    }

    public void writeArrayHeader(int n) {
        header(n, 0x90, 15, 0, 0xdc, 0xdd);
    }

    public void writeMapHeader(int n) {
        header(n, 0x80, 15, 0, 0xde, 0xdf);
    }
}
