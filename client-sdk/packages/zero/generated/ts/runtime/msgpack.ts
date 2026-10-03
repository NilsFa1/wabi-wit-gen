// Copied by wabi-wit-gen from generator/runtime — DO NOT EDIT; change the source there.
// Hand-written, generic: the strict MessagePack subset of payload encoding v1
// (abi/ABI.md §5). No dependency on a MessagePack library.
import { WabiProtocolException } from "./errors.ts";

const utf8Encoder = new TextEncoder();
const utf8Decoder = new TextDecoder("utf-8", { fatal: true });

export class MsgPackWriter {
  private buf = new Uint8Array(64);
  private view = new DataView(this.buf.buffer);
  private len = 0;

  /** The encoded bytes (a copy-free view; the writer must not be reused). */
  bytes(): Uint8Array {
    return this.buf.subarray(0, this.len);
  }

  private reserve(n: number): void {
    if (this.len + n <= this.buf.length) return;
    let size = this.buf.length * 2;
    while (size < this.len + n) size *= 2;
    const next = new Uint8Array(size);
    next.set(this.buf.subarray(0, this.len));
    this.buf = next;
    this.view = new DataView(next.buffer);
  }

  private byte(v: number): void {
    this.reserve(1);
    this.buf[this.len++] = v;
  }

  private be(v: number | bigint, n: number): void {
    this.reserve(n);
    switch (n) {
      case 1: this.view.setUint8(this.len, Number(v)); break;
      case 2: this.view.setUint16(this.len, Number(v)); break;
      case 4: this.view.setUint32(this.len, Number(v)); break;
      default: this.view.setBigUint64(this.len, BigInt.asUintN(64, BigInt(v)));
    }
    this.len += n;
  }

  writeNil(): void {
    this.byte(0xc0);
  }

  writeBool(v: boolean): void {
    this.byte(v ? 0xc3 : 0xc2);
  }

  /** Unsigned integer (u8…u64), smallest encoding. */
  writeUint(v: number | bigint): void {
    const b = BigInt(v);
    if (b < 0n || b > 0xffffffffffffffffn) throw new RangeError(`unsigned integer out of range: ${v}`);
    if (b <= 0x7fn) this.byte(Number(b));
    else if (b <= 0xffn) { this.byte(0xcc); this.be(b, 1); }
    else if (b <= 0xffffn) { this.byte(0xcd); this.be(b, 2); }
    else if (b <= 0xffffffffn) { this.byte(0xce); this.be(b, 4); }
    else { this.byte(0xcf); this.be(b, 8); }
  }

  /** Signed integer (s8…s64), smallest encoding. */
  writeInt(v: number | bigint): void {
    const b = BigInt(v);
    if (b >= 0n) return this.writeUint(b);
    if (b < -0x8000000000000000n) throw new RangeError(`signed integer out of range: ${v}`);
    if (b >= -32n) this.byte(Number(b) & 0xff);
    else if (b >= -0x80n) { this.byte(0xd0); this.be(Number(b) & 0xff, 1); }
    else if (b >= -0x8000n) { this.byte(0xd1); this.be(Number(b) & 0xffff, 2); }
    else if (b >= -0x80000000n) { this.byte(0xd2); this.be(Number(b) >>> 0, 4); }
    else { this.byte(0xd3); this.be(b, 8); }
  }

  writeFloat32(v: number): void {
    this.byte(0xca);
    this.reserve(4);
    this.view.setFloat32(this.len, v);
    this.len += 4;
  }

  writeFloat64(v: number): void {
    this.byte(0xcb);
    this.reserve(8);
    this.view.setFloat64(this.len, v);
    this.len += 8;
  }

  private header(n: number, fixBase: number | null, fixMax: number, m8: number | null, m16: number, m32: number): void {
    if (!Number.isInteger(n) || n < 0 || n > 0xffffffff) throw new RangeError(`length out of range: ${n}`);
    if (fixBase !== null && n <= fixMax) this.byte(fixBase | n);
    else if (m8 !== null && n <= 0xff) { this.byte(m8); this.be(n, 1); }
    else if (n <= 0xffff) { this.byte(m16); this.be(n, 2); }
    else { this.byte(m32); this.be(n, 4); }
  }

  /** Always UTF-8; lone surrogates are rejected, never replaced. */
  writeString(s: string): void {
    if (!s.isWellFormed()) throw new TypeError("string contains an unpaired surrogate");
    const bytes = utf8Encoder.encode(s);
    this.header(bytes.length, 0xa0, 31, 0xd9, 0xda, 0xdb);
    this.reserve(bytes.length);
    this.buf.set(bytes, this.len);
    this.len += bytes.length;
  }

  writeBinary(v: Uint8Array): void {
    this.header(v.length, null, 0, 0xc4, 0xc5, 0xc6);
    this.reserve(v.length);
    this.buf.set(v, this.len);
    this.len += v.length;
  }

  writeArrayHeader(n: number): void {
    this.header(n, 0x90, 15, null, 0xdc, 0xdd);
  }

  writeMapHeader(n: number): void {
    this.header(n, 0x80, 15, null, 0xde, 0xdf);
  }
}

const MAX_DEPTH = 64;

export class MsgPackReader {
  private pos = 0;
  private readonly view: DataView;

  constructor(private readonly buf: Uint8Array) {
    this.view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  }

  get remaining(): number {
    return this.buf.length - this.pos;
  }

  private static eof(): WabiProtocolException {
    return new WabiProtocolException("unexpected end of payload");
  }

  private static unexpected(what: string, m: number): WabiProtocolException {
    return new WabiProtocolException(`expected ${what}, found marker 0x${m.toString(16)}`);
  }

  private peek(): number {
    if (this.pos >= this.buf.length) throw MsgPackReader.eof();
    return this.buf[this.pos]!;
  }

  private next(): number {
    if (this.pos >= this.buf.length) throw MsgPackReader.eof();
    return this.buf[this.pos++]!;
  }

  private need(n: number): void {
    if (n < 0 || n > this.remaining) throw new WabiProtocolException("length exceeds payload");
  }

  private be(n: 1 | 2 | 4): number {
    this.need(n);
    const v = n === 1 ? this.view.getUint8(this.pos) : n === 2 ? this.view.getUint16(this.pos) : this.view.getUint32(this.pos);
    this.pos += n;
    return v;
  }

  private take(n: number): Uint8Array {
    this.need(n);
    const s = this.buf.subarray(this.pos, this.pos + n);
    this.pos += n;
    return s;
  }

  tryReadNil(): boolean {
    if (this.peek() !== 0xc0) return false;
    this.pos++;
    return true;
  }

  readNil(): void {
    const m = this.next();
    if (m !== 0xc0) throw MsgPackReader.unexpected("nil", m);
  }

  readBool(): boolean {
    const m = this.next();
    if (m === 0xc2) return false;
    if (m === 0xc3) return true;
    throw MsgPackReader.unexpected("bool", m);
  }

  /** Any integer encoding as bigint (exact for the whole u64/s64 range). */
  private integer(): bigint {
    const m = this.next();
    if (m <= 0x7f) return BigInt(m);
    if (m >= 0xe0) return BigInt(m - 0x100);
    switch (m) {
      case 0xcc: return BigInt(this.be(1));
      case 0xcd: return BigInt(this.be(2));
      case 0xce: return BigInt(this.be(4));
      case 0xcf: { this.need(8); const v = this.view.getBigUint64(this.pos); this.pos += 8; return v; }
      case 0xd0: { this.need(1); const v = this.view.getInt8(this.pos); this.pos += 1; return BigInt(v); }
      case 0xd1: { this.need(2); const v = this.view.getInt16(this.pos); this.pos += 2; return BigInt(v); }
      case 0xd2: { this.need(4); const v = this.view.getInt32(this.pos); this.pos += 4; return BigInt(v); }
      case 0xd3: { this.need(8); const v = this.view.getBigInt64(this.pos); this.pos += 8; return v; }
      default: throw MsgPackReader.unexpected("integer", m);
    }
  }

  /** u64 (handles, ids): exact as bigint. */
  readUint64(): bigint {
    const v = this.integer();
    if (v < 0n) throw new WabiProtocolException("negative value for unsigned integer");
    return v;
  }

  /** u8/u16/u32/char: range-checked number. */
  readUintMax(max: number): number {
    const v = this.readUint64();
    if (v > BigInt(max)) throw new WabiProtocolException(`integer ${v} out of range`);
    return Number(v);
  }

  /** s64: exact as bigint. */
  readInt64(): bigint {
    const v = this.integer();
    if (v > 0x7fffffffffffffffn) throw new WabiProtocolException("integer out of range for s64");
    return v;
  }

  /** s8/s16/s32: range-checked number. */
  readIntRange(min: number, max: number): number {
    const v = this.readInt64();
    if (v < BigInt(min) || v > BigInt(max)) throw new WabiProtocolException(`integer ${v} out of range`);
    return Number(v);
  }

  readFloat(): number {
    const m = this.next();
    if (m === 0xca) { this.need(4); const v = this.view.getFloat32(this.pos); this.pos += 4; return v; }
    if (m === 0xcb) { this.need(8); const v = this.view.getFloat64(this.pos); this.pos += 8; return v; }
    throw MsgPackReader.unexpected("float", m);
  }

  private strLen(): number {
    const m = this.next();
    if (m >= 0xa0 && m <= 0xbf) return m & 0x1f;
    if (m === 0xd9) return this.be(1);
    if (m === 0xda) return this.be(2);
    if (m === 0xdb) return this.be(4);
    throw MsgPackReader.unexpected("string", m);
  }

  /** Invalid UTF-8 is rejected, never replaced. */
  readString(): string {
    const bytes = this.take(this.strLen());
    try {
      return utf8Decoder.decode(bytes);
    } catch {
      throw new WabiProtocolException("string is not valid UTF-8");
    }
  }

  /** A copy (safe to keep after the reader is gone). */
  readBinary(): Uint8Array {
    const m = this.next();
    const n = m === 0xc4 ? this.be(1) : m === 0xc5 ? this.be(2) : m === 0xc6 ? this.be(4) : -1;
    if (n < 0) throw MsgPackReader.unexpected("bin", m);
    return this.take(n).slice();
  }

  readArrayHeader(): number {
    const m = this.next();
    const n = m >= 0x90 && m <= 0x9f ? m & 0x0f : m === 0xdc ? this.be(2) : m === 0xdd ? this.be(4) : -1;
    if (n < 0) throw MsgPackReader.unexpected("array", m);
    if (n > this.remaining) throw new WabiProtocolException("array length exceeds payload");
    return n;
  }

  readMapHeader(): number {
    const m = this.next();
    const n = m >= 0x80 && m <= 0x8f ? m & 0x0f : m === 0xde ? this.be(2) : m === 0xdf ? this.be(4) : -1;
    if (n < 0) throw MsgPackReader.unexpected("map", m);
    if (n * 2 > this.remaining) throw new WabiProtocolException("map length exceeds payload");
    return n;
  }

  readTupleHeader(expected: number): void {
    const n = this.readArrayHeader();
    if (n !== expected) throw new WabiProtocolException(`expected tuple of ${expected}, found ${n}`);
  }

  readList<T>(item: (r: MsgPackReader) => T): T[] {
    const n = this.readArrayHeader();
    const out: T[] = [];
    for (let i = 0; i < n; i++) out.push(item(this));
    return out;
  }

  readOptional<T>(item: (r: MsgPackReader) => T): T | undefined {
    return this.tryReadNil() ? undefined : item(this);
  }

  /** `result<T, E>` is `[tag, value]`: 0 = ok, 1 = err. */
  readResultTag(): number {
    const n = this.readArrayHeader();
    if (n !== 2) throw new WabiProtocolException(`result must have 2 elements, found ${n}`);
    const tag = this.readUint64();
    if (tag > 1n) throw new WabiProtocolException(`invalid result tag ${tag}`);
    return Number(tag);
  }

  /** Skips one value (unknown record fields, extra variant elements). */
  skip(depth = 0): void {
    if (depth > MAX_DEPTH) throw new WabiProtocolException("payload nested too deeply");
    const m = this.peek();
    if (m <= 0x7f || m >= 0xe0 || (m >= 0xcc && m <= 0xd3)) this.integer();
    else if (m === 0xc0 || m === 0xc2 || m === 0xc3) this.pos++;
    else if (m === 0xca || m === 0xcb) this.readFloat();
    else if ((m >= 0xa0 && m <= 0xbf) || (m >= 0xd9 && m <= 0xdb)) this.take(this.strLen());
    else if (m >= 0xc4 && m <= 0xc6) this.readBinary();
    else if ((m >= 0x90 && m <= 0x9f) || m === 0xdc || m === 0xdd) {
      for (let i = this.readArrayHeader(); i > 0; i--) this.skip(depth + 1);
    } else if ((m >= 0x80 && m <= 0x8f) || m === 0xde || m === 0xdf) {
      for (let i = this.readMapHeader() * 2; i > 0; i--) this.skip(depth + 1);
    } else if (m >= 0xd4 && m <= 0xd8) {
      this.pos++;
      this.take(1 + (1 << (m - 0xd4)));
    } else if (m >= 0xc7 && m <= 0xc9) {
      this.pos++;
      this.take(1 + this.be(m === 0xc7 ? 1 : m === 0xc8 ? 2 : 4));
    } else throw MsgPackReader.unexpected("value", m);
  }
}
