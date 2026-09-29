//! Minimal Zig core implementing the Acme native ABI v1 (abi/acme_abi_functions.h).
//!
//! Deliberately a different architecture from the Rust and Go cores: it is
//! single-threaded and *eager* — every future is born complete, so
//! acme_future_subscribe always returns ACME_READY and no callback is ever
//! invoked. The ABI allows this (capability THREADED_CALLBACK is not set), and
//! the unchanged SDKs handle it, which is the point of this stub.
//!
//! Scope (enough for the "basic" conformance profile): constructor, state,
//! connect/disconnect (mem:// + api key), get-customer, events (ends at once).
//!
//!   zig build-lib -dynamic -lc -O ReleaseSafe acme_core.zig -femit-bin=libacme_client.so
//!
//! Exported symbols carry the package prefix from acme_prefix.zig
//! (acme_client_call_sync, ...).

const std = @import("std");
const c = std.c;

// ------------------------------------------------------------------ C types

const AcmeBytes = extern struct { ptr: ?[*]const u8, len: usize };
const AcmeBuffer = extern struct { ptr: ?[*]u8, len: usize };
const Callback = ?*const fn (?*anyopaque, u64) callconv(.c) void;

const OK: i32 = 0;
const PENDING: i32 = 1;
const READY: i32 = 2;
const STREAM_END: i32 = 3;
const ERR_INVALID_HANDLE: i32 = -1;
const ERR_INVALID_ARGUMENT: i32 = -2;
const ERR_UNKNOWN_METHOD: i32 = -3;
const ERR_CANCELLED: i32 = -4;
const ERR_BUSY: i32 = -5;
const ERR_ALREADY_TAKEN: i32 = -6;
const ERR_UNSUPPORTED: i32 = -7;
const ERR_INTERNAL: i32 = -8;

// Capabilities: sync calls, async calls, streams, payload v1. Not
// CANCELLATION / THREADED_CALLBACK: nothing is ever in flight.
const CAPS: u64 = (1 << 0) | (1 << 1) | (1 << 2) | (1 << 5);

// Method ids from wit/abi-ids.lock.
const M_CTOR: u32 = 0x0100;
const M_STATE: u32 = 0x0101;
const M_CONNECT: u32 = 0x0102;
const M_DISCONNECT: u32 = 0x0103;
const M_GET_CUSTOMER: u32 = 0x0104;
const M_CREATE_CUSTOMER: u32 = 0x0105;
const M_SEND_COMMAND: u32 = 0x0106;
const M_EVENTS: u32 = 0x0107;

// ------------------------------------------------------------- handle table

const Client = struct { endpoint: []u8, api_key: ?[]u8, state: u64 = 1 };
const Future = struct { status: i32, payload: []u8, taken: bool = false, subscribed: bool = false };
const Entry = union(enum) { object: *Client, future: *Future, stream: void };

const alloc = std.heap.c_allocator;
var mutex: c.pthread_mutex_t = c.PTHREAD_MUTEX_INITIALIZER;
var table: std.AutoHashMapUnmanaged(u64, Entry) = .empty;
var serial: u64 = 0;

fn lock() void {
    _ = c.pthread_mutex_lock(&mutex);
}
fn unlock() void {
    _ = c.pthread_mutex_unlock(&mutex);
}

/// Caller holds the lock. Handles are never reused (monotonic serial).
fn insert(kind: u64, e: Entry) !u64 {
    serial += 1;
    const h = (kind << 56) | serial;
    try table.put(alloc, h, e);
    return h;
}

// ------------------------------------------------------------ msgpack (v1)

const Writer = struct {
    buf: [512]u8 = undefined,
    len: usize = 0,

    fn b(w: *Writer, v: u8) void {
        w.buf[w.len] = v;
        w.len += 1;
    }
    fn be(w: *Writer, v: u64, n: usize) void {
        var i: usize = n;
        while (i > 0) : (i -= 1) w.b(@truncate(v >> @intCast(8 * (i - 1))));
    }
    fn uint(w: *Writer, v: u64) void {
        if (v < 0x80) {
            w.b(@truncate(v));
        } else if (v <= 0xff) {
            w.b(0xcc);
            w.be(v, 1);
        } else if (v <= 0xffff) {
            w.b(0xcd);
            w.be(v, 2);
        } else if (v <= 0xffff_ffff) {
            w.b(0xce);
            w.be(v, 4);
        } else {
            w.b(0xcf);
            w.be(v, 8);
        }
    }
    fn str(w: *Writer, s: []const u8) void {
        if (s.len <= 31) w.b(0xa0 | @as(u8, @truncate(s.len))) else {
            w.b(0xd9);
            w.b(@truncate(s.len));
        }
        @memcpy(w.buf[w.len..][0..s.len], s);
        w.len += s.len;
    }
    fn bytes(w: *const Writer) []const u8 {
        return w.buf[0..w.len];
    }
};

const Reader = struct {
    b: []const u8,
    pos: usize = 0,

    fn byte(r: *Reader) !u8 {
        if (r.pos >= r.b.len) return error.Malformed;
        r.pos += 1;
        return r.b[r.pos - 1];
    }
    fn be(r: *Reader, n: usize) !u64 {
        var v: u64 = 0;
        for (0..n) |_| v = (v << 8) | try r.byte();
        return v;
    }
    fn uint(r: *Reader) !u64 {
        const m = try r.byte();
        return switch (m) {
            0x00...0x7f => m,
            0xcc => r.be(1),
            0xcd => r.be(2),
            0xce => r.be(4),
            0xcf => r.be(8),
            else => error.Malformed,
        };
    }
    fn str(r: *Reader) ![]const u8 {
        const m = try r.byte();
        const n: usize = switch (m) {
            0xa0...0xbf => m & 0x1f,
            0xd9 => @intCast(try r.be(1)),
            0xda => @intCast(try r.be(2)),
            0xdb => @intCast(try r.be(4)),
            else => return error.Malformed,
        };
        if (n > r.b.len - r.pos) return error.Malformed;
        const s = r.b[r.pos .. r.pos + n];
        r.pos += n;
        if (!std.unicode.utf8ValidateSlice(s)) return error.Malformed;
        return s;
    }
    fn header(r: *Reader, fix_lo: u8, fix_hi: u8, m16: u8) !usize {
        const m = try r.byte();
        if (m >= fix_lo and m <= fix_hi) return m & 0x0f;
        if (m == m16) return @intCast(try r.be(2));
        return error.Malformed;
    }
    fn array(r: *Reader) !usize {
        return r.header(0x90, 0x9f, 0xdc);
    }
    fn map(r: *Reader) !usize {
        return r.header(0x80, 0x8f, 0xde);
    }
    fn isNil(r: *Reader) bool {
        return r.pos < r.b.len and r.b[r.pos] == 0xc0;
    }
    fn done(r: *const Reader) bool {
        return r.pos == r.b.len;
    }
};

// ------------------------------------------------------------------ helpers

fn toBuffer(data: []const u8) AcmeBuffer {
    if (data.len == 0) return .{ .ptr = null, .len = 0 };
    const mem = alloc.alloc(u8, data.len) catch return .{ .ptr = null, .len = 0 };
    @memcpy(mem, data);
    return .{ .ptr = mem.ptr, .len = mem.len };
}

fn requestSlice(req: AcmeBytes) ?[]const u8 {
    if (req.len == 0) return &[_]u8{};
    const p = req.ptr orelse return null;
    return p[0..req.len];
}

fn resultErr(w: *Writer, tag: u64, msg: ?[]const u8) void {
    w.b(0x92); // [1, client-error]
    w.b(0x01);
    if (msg) |m| {
        w.b(0x92);
        w.uint(tag);
        w.str(m);
    } else {
        w.b(0x91);
        w.uint(tag);
    }
}

fn startsWith(s: []const u8, prefix: []const u8) bool {
    return s.len >= prefix.len and std.mem.eql(u8, s[0..prefix.len], prefix);
}

// ------------------------------------------------------------------ exports

/// Package symbol prefix (ABI.md §1.1): acme:client -> acme_client_call_sync, ...
/// Kept in its own file so the same core can be built for another package.
pub const symbol_prefix = @import("acme_prefix.zig").value;

comptime {
    @export(&abi_version, .{ .name = symbol_prefix ++ "_abi_version" });
    @export(&abi_capabilities, .{ .name = symbol_prefix ++ "_abi_capabilities" });
    @export(&contract_id, .{ .name = symbol_prefix ++ "_contract_id" });
    @export(&core_info, .{ .name = symbol_prefix ++ "_core_info" });
    @export(&status_name, .{ .name = symbol_prefix ++ "_status_name" });
    @export(&buffer_free, .{ .name = symbol_prefix ++ "_buffer_free" });
    @export(&call_sync, .{ .name = symbol_prefix ++ "_call_sync" });
    @export(&call_async, .{ .name = symbol_prefix ++ "_call_async" });
    @export(&object_drop, .{ .name = symbol_prefix ++ "_object_drop" });
    @export(&future_subscribe, .{ .name = symbol_prefix ++ "_future_subscribe" });
    @export(&future_poll, .{ .name = symbol_prefix ++ "_future_poll" });
    @export(&future_take, .{ .name = symbol_prefix ++ "_future_take" });
    @export(&future_cancel, .{ .name = symbol_prefix ++ "_future_cancel" });
    @export(&future_drop, .{ .name = symbol_prefix ++ "_future_drop" });
    @export(&stream_next, .{ .name = symbol_prefix ++ "_stream_next" });
    @export(&stream_cancel, .{ .name = symbol_prefix ++ "_stream_cancel" });
    @export(&stream_drop, .{ .name = symbol_prefix ++ "_stream_drop" });
}

// ------------------------------------------------------------------ metadata

fn abi_version() callconv(.c) u32 {
    return (1 << 16) | 0;
}
fn abi_capabilities() callconv(.c) u64 {
    return CAPS;
}
fn contract_id() callconv(.c) [*:0]const u8 {
    return "acme:client@1.0.0";
}
fn core_info() callconv(.c) [*:0]const u8 {
    return "acme-zig-core/0.1.0 (eager stub)";
}
fn status_name(status: i32) callconv(.c) [*:0]const u8 {
    return switch (status) {
        OK => "ACME_OK",
        PENDING => "ACME_PENDING",
        READY => "ACME_READY",
        STREAM_END => "ACME_STREAM_END",
        ERR_INVALID_HANDLE => "ACME_ERR_INVALID_HANDLE",
        ERR_INVALID_ARGUMENT => "ACME_ERR_INVALID_ARGUMENT",
        ERR_UNKNOWN_METHOD => "ACME_ERR_UNKNOWN_METHOD",
        ERR_CANCELLED => "ACME_ERR_CANCELLED",
        ERR_BUSY => "ACME_ERR_BUSY",
        ERR_ALREADY_TAKEN => "ACME_ERR_ALREADY_TAKEN",
        ERR_UNSUPPORTED => "ACME_ERR_UNSUPPORTED",
        ERR_INTERNAL => "ACME_ERR_INTERNAL",
        else => "ACME_UNKNOWN_STATUS",
    };
}

fn buffer_free(buf: AcmeBuffer) callconv(.c) void {
    if (buf.ptr) |p| alloc.free(p[0..buf.len]);
}

// --------------------------------------------------------------------- calls

fn call_sync(object: u64, method: u32, request: AcmeBytes, out: ?*AcmeBuffer) callconv(.c) i32 {
    const o = out orelse return ERR_INVALID_ARGUMENT;
    o.* = .{ .ptr = null, .len = 0 };
    const req = requestSlice(request) orelse return ERR_INVALID_ARGUMENT;
    lock();
    defer unlock();
    var w = Writer{};
    var r = Reader{ .b = req };
    switch (method) {
        M_CTOR => {
            if (object != 0) return ERR_INVALID_HANDLE;
            return constructor(&r, &w, o);
        },
        M_STATE, M_EVENTS => {
            const e = table.get(object) orelse return ERR_INVALID_HANDLE;
            if (e != .object) return ERR_INVALID_HANDLE;
            if ((r.array() catch return ERR_INVALID_ARGUMENT) != 0 or !r.done()) return ERR_INVALID_ARGUMENT;
            if (method == M_STATE) {
                w.uint(e.object.state);
            } else {
                // An eager core has no event source: the stream ends immediately.
                w.uint(insert(3, .{ .stream = {} }) catch return ERR_INTERNAL);
            }
        },
        M_CONNECT, M_DISCONNECT, M_GET_CUSTOMER, M_CREATE_CUSTOMER, M_SEND_COMMAND => {
            if (table.get(object) == null) return ERR_INVALID_HANDLE;
            return ERR_UNSUPPORTED;
        },
        else => return ERR_UNKNOWN_METHOD,
    }
    o.* = toBuffer(w.bytes());
    return OK;
}

fn constructor(r: *Reader, w: *Writer, out: *AcmeBuffer) i32 {
    const bad = "malformed client-options";
    if ((r.array() catch 0) != 1) {
        out.* = toBuffer(bad);
        return ERR_INVALID_ARGUMENT;
    }
    const n = r.map() catch {
        out.* = toBuffer(bad);
        return ERR_INVALID_ARGUMENT;
    };
    var endpoint: ?[]const u8 = null;
    var api_key: ?[]const u8 = null;
    var have_timeout = false;
    for (0..n) |_| {
        const tag = r.uint() catch break;
        switch (tag) {
            1 => endpoint = r.str() catch null,
            2 => if (r.isNil()) {
                r.pos += 1;
            } else {
                api_key = r.str() catch null;
            },
            3 => have_timeout = if (r.uint()) |_| true else |_| false,
            else => break, // this stub does not skip unknown fields
        }
    }
    if (endpoint == null or !have_timeout or !r.done()) {
        out.* = toBuffer(bad);
        return ERR_INVALID_ARGUMENT;
    }
    const client = alloc.create(Client) catch return ERR_INTERNAL;
    client.* = .{
        .endpoint = alloc.dupe(u8, endpoint.?) catch return ERR_INTERNAL,
        .api_key = if (api_key) |k| (alloc.dupe(u8, k) catch return ERR_INTERNAL) else null,
    };
    w.uint(insert(1, .{ .object = client }) catch return ERR_INTERNAL);
    out.* = toBuffer(w.bytes());
    return OK;
}

fn call_async(object: u64, method: u32, request: AcmeBytes, out: ?*u64) callconv(.c) i32 {
    const o = out orelse return ERR_INVALID_ARGUMENT;
    o.* = 0;
    const req = requestSlice(request) orelse return ERR_INVALID_ARGUMENT;
    lock();
    defer unlock();
    if (method == M_CTOR or method == M_STATE or method == M_EVENTS) return ERR_UNSUPPORTED;
    if (method < M_CONNECT or method > M_SEND_COMMAND) return ERR_UNKNOWN_METHOD;
    const e = table.get(object) orelse return ERR_INVALID_HANDLE;
    if (e != .object) return ERR_INVALID_HANDLE;
    const client = e.object;

    var w = Writer{};
    var r = Reader{ .b = req };
    const argc = r.array() catch return ERR_INVALID_ARGUMENT;
    switch (method) {
        M_CONNECT => {
            if (argc != 0) return ERR_INVALID_ARGUMENT;
            if (std.mem.eql(u8, client.endpoint, "mem://unreachable")) {
                resultErr(&w, 4, "connection refused");
            } else if (!startsWith(client.endpoint, "mem://")) {
                resultErr(&w, 4, "zig stub supports mem:// endpoints only");
            } else if (client.api_key == null or !std.mem.eql(u8, client.api_key.?, "demo-key")) {
                resultErr(&w, 2, null);
            } else {
                client.state = 3;
                w.b(0x92);
                w.b(0x00);
                w.b(0xc0);
            }
        },
        M_DISCONNECT => {
            if (argc != 0) return ERR_INVALID_ARGUMENT;
            client.state = 1;
            w.b(0x92);
            w.b(0x00);
            w.b(0xc0);
        },
        M_GET_CUSTOMER => {
            if (argc != 1) return ERR_INVALID_ARGUMENT;
            const id = r.str() catch return ERR_INVALID_ARGUMENT;
            if (client.state != 3) {
                resultErr(&w, 3, null);
            } else if (std.mem.eql(u8, id, "4711")) {
                w.b(0x92);
                w.b(0x00);
                w.b(0x83);
                w.uint(1);
                w.str("4711");
                w.uint(2);
                w.str("Erika Mustermann");
                w.uint(3);
                w.str("erika@example.com");
            } else {
                resultErr(&w, 1, null);
            }
        },
        else => resultErr(&w, 5, "not implemented by the zig stub core"),
    }
    const f = alloc.create(Future) catch return ERR_INTERNAL;
    f.* = .{ .status = OK, .payload = alloc.dupe(u8, w.bytes()) catch return ERR_INTERNAL };
    o.* = insert(2, .{ .future = f }) catch return ERR_INTERNAL;
    return OK;
}

fn object_drop(object: u64) callconv(.c) i32 {
    lock();
    defer unlock();
    const e = table.get(object) orelse return ERR_INVALID_HANDLE;
    if (e != .object) return ERR_INVALID_HANDLE;
    _ = table.fetchRemove(object);
    alloc.free(e.object.endpoint);
    if (e.object.api_key) |k| alloc.free(k);
    alloc.destroy(e.object);
    return OK;
}

// ------------------------------------------------------------------- futures

fn futureOf(h: u64) ?*Future {
    const e = table.get(h) orelse return null;
    return if (e == .future) e.future else null;
}

fn future_subscribe(h: u64, cb: Callback, ud: ?*anyopaque) callconv(.c) i32 {
    _ = ud;
    if (cb == null) return ERR_INVALID_ARGUMENT;
    lock();
    defer unlock();
    const f = futureOf(h) orelse return ERR_INVALID_HANDLE;
    if (f.subscribed) return ERR_BUSY;
    f.subscribed = true;
    return READY; // eager core: always complete, the callback is never invoked
}

fn future_poll(h: u64) callconv(.c) i32 {
    lock();
    defer unlock();
    _ = futureOf(h) orelse return ERR_INVALID_HANDLE;
    return READY;
}

fn future_take(h: u64, out: ?*AcmeBuffer) callconv(.c) i32 {
    const o = out orelse return ERR_INVALID_ARGUMENT;
    o.* = .{ .ptr = null, .len = 0 };
    lock();
    defer unlock();
    const f = futureOf(h) orelse return ERR_INVALID_HANDLE;
    if (f.taken) return ERR_ALREADY_TAKEN;
    f.taken = true;
    o.* = toBuffer(f.payload);
    return f.status;
}

fn future_cancel(h: u64) callconv(.c) i32 {
    lock();
    defer unlock();
    _ = futureOf(h) orelse return ERR_INVALID_HANDLE;
    return OK; // already complete: completion wins
}

fn future_drop(h: u64) callconv(.c) i32 {
    lock();
    defer unlock();
    const f = futureOf(h) orelse return ERR_INVALID_HANDLE;
    _ = table.fetchRemove(h);
    alloc.free(f.payload);
    alloc.destroy(f);
    return OK;
}

// ------------------------------------------------------------------- streams

fn isStream(h: u64) bool {
    const e = table.get(h) orelse return false;
    return e == .stream;
}

fn stream_next(h: u64, out: ?*u64) callconv(.c) i32 {
    const o = out orelse return ERR_INVALID_ARGUMENT;
    o.* = 0;
    lock();
    defer unlock();
    if (!isStream(h)) return ERR_INVALID_HANDLE;
    const f = alloc.create(Future) catch return ERR_INTERNAL;
    f.* = .{ .status = STREAM_END, .payload = &[_]u8{} };
    o.* = insert(2, .{ .future = f }) catch return ERR_INTERNAL;
    return OK;
}

fn stream_cancel(h: u64) callconv(.c) i32 {
    lock();
    defer unlock();
    return if (isStream(h)) OK else ERR_INVALID_HANDLE;
}

fn stream_drop(h: u64) callconv(.c) i32 {
    lock();
    defer unlock();
    if (!isStream(h)) return ERR_INVALID_HANDLE;
    _ = table.fetchRemove(h);
    return OK;
}
