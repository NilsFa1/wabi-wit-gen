// The TypeScript runtime's payload codec (abi/ABI.md §5), independent of any core.
import { expect, test } from "bun:test";
import { WabiProtocolException, MsgPackReader, MsgPackWriter } from "../../../generated/ts/runtime/index.ts";

const roundtrip = (write: (w: MsgPackWriter) => void) => {
  const w = new MsgPackWriter();
  write(w);
  return new MsgPackReader(w.bytes().slice());
};

test("unsigned integers use the smallest encoding and keep u64 exact", () => {
  const cases: [bigint, number][] = [
    [0n, 1], [0x7fn, 1], [0x80n, 2], [0xffn, 2], [0x100n, 3], [0xffffn, 3],
    [0x10000n, 5], [0xffffffffn, 5], [0x100000000n, 9], [0xffffffffffffffffn, 9],
  ];
  for (const [v, size] of cases) {
    const w = new MsgPackWriter();
    w.writeUint(v);
    expect(w.bytes().length).toBe(size);
    expect(new MsgPackReader(w.bytes().slice()).readUint64()).toBe(v);
  }
  expect(() => new MsgPackWriter().writeUint(-1)).toThrow(RangeError);
  expect(() => new MsgPackWriter().writeUint(1n << 64n)).toThrow(RangeError);
});

test("signed integers and range checks", () => {
  for (const v of [-1n, -32n, -33n, -128n, -129n, -32768n, -32769n, -0x80000000n, -0x80000001n, -(1n << 63n), (1n << 63n) - 1n]) {
    expect(roundtrip((w) => w.writeInt(v)).readInt64()).toBe(v);
  }
  expect(roundtrip((w) => w.writeInt(-129)).readIntRange(-0x8000, 0x7fff)).toBe(-129);
  expect(() => roundtrip((w) => w.writeInt(-129)).readIntRange(-0x80, 0x7f)).toThrow(WabiProtocolException);
  expect(() => roundtrip((w) => w.writeUint(256)).readUintMax(0xff)).toThrow(WabiProtocolException);
  expect(() => roundtrip((w) => w.writeInt(-1)).readUint64()).toThrow(WabiProtocolException);
});

test("strings are UTF-8, lone surrogates are rejected on both sides", () => {
  for (const s of ["", "a", "Grüße 🩺 – 漢字", "x".repeat(31), "x".repeat(32), "y".repeat(70000)]) {
    expect(roundtrip((w) => w.writeString(s)).readString()).toBe(s);
  }
  expect(() => new MsgPackWriter().writeString("\ud800")).toThrow(TypeError);
  const invalid = new MsgPackReader(Uint8Array.from([0xa2, 0xc3, 0x28]));
  expect(() => invalid.readString()).toThrow(WabiProtocolException);
});

test("binary, floats, nil, containers and skip", () => {
  const big = new Uint8Array(4 * 1024 * 1024).map((_, i) => i & 0xff);
  expect(roundtrip((w) => w.writeBinary(big)).readBinary()).toEqual(big);
  expect(roundtrip((w) => w.writeFloat64(Math.PI)).readFloat()).toBe(Math.PI);
  expect(roundtrip((w) => w.writeFloat32(1.5)).readFloat()).toBe(1.5);

  const r = roundtrip((w) => {
    w.writeMapHeader(2);
    w.writeUint(9); // unknown field: skipped
    w.writeArrayHeader(3); w.writeString("x"); w.writeNil(); w.writeMapHeader(1); w.writeUint(1); w.writeBool(true);
    w.writeUint(1); w.writeString("kept");
  });
  const n = r.readMapHeader();
  let kept = "";
  for (let i = 0; i < n; i++) {
    if (r.readUint64() === 1n) kept = r.readString();
    else r.skip();
  }
  expect(kept).toBe("kept");
  expect(r.remaining).toBe(0);
});

test("malformed payloads fail cleanly", () => {
  expect(() => new MsgPackReader(new Uint8Array()).readUint64()).toThrow("unexpected end of payload");
  expect(() => new MsgPackReader(Uint8Array.from([0xdd, 0xff, 0xff, 0xff, 0xff])).readArrayHeader()).toThrow(WabiProtocolException);
  let deep = new Uint8Array(100).fill(0x91);
  expect(() => new MsgPackReader(deep).skip()).toThrow(WabiProtocolException);
  expect(() => new MsgPackReader(Uint8Array.from([0x92, 0x02, 0xc0])).readResultTag()).toThrow("invalid result tag");
});
