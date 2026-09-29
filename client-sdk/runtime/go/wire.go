package acmeabi

// Payload encoding v1 (ABI.md §5): the MessagePack subset every SDK runtime
// and every core implements. The Reader has a sticky error so generated
// decoders stay straight-line code: decode everything, then check r.Err once.

import (
	"encoding/binary"
	"errors"
	"fmt"
	"math"
	"unicode/utf8"
)

// Writer appends MessagePack values to B.
type Writer struct{ B []byte }

func (w *Writer) Nil() { w.B = append(w.B, 0xc0) }

func (w *Writer) Bool(v bool) {
	if v {
		w.B = append(w.B, 0xc3)
	} else {
		w.B = append(w.B, 0xc2)
	}
}

func (w *Writer) Uint(v uint64) {
	switch {
	case v < 0x80:
		w.B = append(w.B, byte(v))
	case v <= math.MaxUint8:
		w.B = append(w.B, 0xcc, byte(v))
	case v <= math.MaxUint16:
		w.B = binary.BigEndian.AppendUint16(append(w.B, 0xcd), uint16(v))
	case v <= math.MaxUint32:
		w.B = binary.BigEndian.AppendUint32(append(w.B, 0xce), uint32(v))
	default:
		w.B = binary.BigEndian.AppendUint64(append(w.B, 0xcf), v)
	}
}

func (w *Writer) Int(v int64) {
	switch {
	case v >= 0:
		w.Uint(uint64(v))
	case v >= -32:
		w.B = append(w.B, byte(v))
	case v >= math.MinInt8:
		w.B = append(w.B, 0xd0, byte(v))
	case v >= math.MinInt16:
		w.B = binary.BigEndian.AppendUint16(append(w.B, 0xd1), uint16(v))
	case v >= math.MinInt32:
		w.B = binary.BigEndian.AppendUint32(append(w.B, 0xd2), uint32(v))
	default:
		w.B = binary.BigEndian.AppendUint64(append(w.B, 0xd3), uint64(v))
	}
}

func (w *Writer) F32(v float32) {
	w.B = binary.BigEndian.AppendUint32(append(w.B, 0xca), math.Float32bits(v))
}

func (w *Writer) F64(v float64) {
	w.B = binary.BigEndian.AppendUint64(append(w.B, 0xcb), math.Float64bits(v))
}

func (w *Writer) header(n int, fixBase byte, fixMax int, m8, m16, m32 byte) {
	switch {
	case fixMax > 0 && n <= fixMax:
		w.B = append(w.B, fixBase|byte(n))
	case m8 != 0 && n <= math.MaxUint8:
		w.B = append(w.B, m8, byte(n))
	case n <= math.MaxUint16:
		w.B = binary.BigEndian.AppendUint16(append(w.B, m16), uint16(n))
	default:
		w.B = binary.BigEndian.AppendUint32(append(w.B, m32), uint32(n))
	}
}

func (w *Writer) Str(s string)  { w.header(len(s), 0xa0, 31, 0xd9, 0xda, 0xdb); w.B = append(w.B, s...) }
func (w *Writer) Bin(v []byte)  { w.header(len(v), 0, 0, 0xc4, 0xc5, 0xc6); w.B = append(w.B, v...) }
func (w *Writer) Array(n int)   { w.header(n, 0x90, 15, 0, 0xdc, 0xdd) }
func (w *Writer) Map(n int)     { w.header(n, 0x80, 15, 0, 0xde, 0xdf) }
func (w *Writer) Bytes() []byte { return w.B }

// ErrPayload is the root of every decoding error.
var ErrPayload = errors.New("malformed payload")

// Reader decodes MessagePack from B. The first error sticks in Err; every
// later read returns a zero value.
type Reader struct {
	B   []byte
	Pos int
	Err error
}

func NewReader(b []byte) *Reader { return &Reader{B: b} }

// Fail records the first error.
func (r *Reader) Fail(format string, args ...any) {
	if r.Err == nil {
		r.Err = fmt.Errorf("%w: %s", ErrPayload, fmt.Sprintf(format, args...))
	}
}

func (r *Reader) next() byte {
	if r.Err != nil {
		return 0
	}
	if r.Pos >= len(r.B) {
		r.Fail("unexpected end")
		return 0
	}
	r.Pos++
	return r.B[r.Pos-1]
}

func (r *Reader) take(n uint64) []byte {
	if r.Err != nil {
		return nil
	}
	if n > uint64(len(r.B)-r.Pos) {
		r.Fail("length %d exceeds payload", n)
		return nil
	}
	s := r.B[r.Pos : r.Pos+int(n)]
	r.Pos += int(n)
	return s
}

func (r *Reader) be(n int) uint64 {
	var v uint64
	for _, c := range r.take(uint64(n)) {
		v = v<<8 | uint64(c)
	}
	return v
}

// IsNil reports (without consuming) whether the next value is nil.
func (r *Reader) IsNil() bool { return r.Err == nil && r.Pos < len(r.B) && r.B[r.Pos] == 0xc0 }

func (r *Reader) Nil() {
	if r.next() != 0xc0 && r.Err == nil {
		r.Fail("expected nil")
	}
}

func (r *Reader) Bool() bool {
	switch r.next() {
	case 0xc2:
		return false
	case 0xc3:
		return true
	}
	r.Fail("expected bool")
	return false
}

// Uint reads an unsigned integer <= max.
func (r *Reader) Uint(max uint64) uint64 {
	m := r.next()
	var v uint64
	switch {
	case r.Err != nil:
		return 0
	case m < 0x80:
		v = uint64(m)
	case m >= 0xcc && m <= 0xcf:
		v = r.be(1 << (m - 0xcc))
	case m >= 0xd0 && m <= 0xd3: // non-negative value in a signed encoding
		i := r.signed(m)
		if i < 0 {
			r.Fail("negative value for unsigned integer")
			return 0
		}
		v = uint64(i)
	default:
		r.Fail("expected unsigned integer")
		return 0
	}
	if v > max {
		r.Fail("integer %d out of range", v)
		return 0
	}
	return v
}

func (r *Reader) signed(m byte) int64 {
	switch m {
	case 0xd0:
		return int64(int8(r.be(1)))
	case 0xd1:
		return int64(int16(r.be(2)))
	case 0xd2:
		return int64(int32(r.be(4)))
	}
	return int64(r.be(8))
}

// Int reads a signed integer within [min, max].
func (r *Reader) Int(min, max int64) int64 {
	if r.Err != nil || r.Pos >= len(r.B) {
		r.next()
		return 0
	}
	m := r.B[r.Pos]
	var v int64
	switch {
	case m >= 0xe0:
		r.Pos++
		v = int64(int8(m))
	case m >= 0xd0 && m <= 0xd3:
		r.Pos++
		v = r.signed(m)
	default:
		u := r.Uint(math.MaxInt64)
		v = int64(u)
	}
	if r.Err == nil && (v < min || v > max) {
		r.Fail("integer %d out of range", v)
		return 0
	}
	return v
}

func (r *Reader) F32() float32 {
	if m := r.next(); m == 0xca {
		return math.Float32frombits(uint32(r.be(4)))
	}
	r.Fail("expected f32")
	return 0
}

func (r *Reader) F64() float64 {
	switch r.next() {
	case 0xcb:
		return math.Float64frombits(r.be(8))
	case 0xca:
		return float64(math.Float32frombits(uint32(r.be(4))))
	}
	r.Fail("expected f64")
	return 0
}

func (r *Reader) Char() rune {
	c := rune(r.Uint(0x10FFFF))
	if c >= 0xD800 && c <= 0xDFFF {
		r.Fail("surrogate is not a char")
	}
	return c
}

func (r *Reader) Str() string {
	m := r.next()
	var n uint64
	switch {
	case r.Err != nil:
		return ""
	case m >= 0xa0 && m <= 0xbf:
		n = uint64(m & 0x1f)
	case m >= 0xd9 && m <= 0xdb:
		n = r.be(1 << (m - 0xd9))
	default:
		r.Fail("expected string")
		return ""
	}
	s := r.take(n)
	if r.Err == nil && !utf8.Valid(s) {
		r.Fail("invalid UTF-8")
	}
	return string(s)
}

func (r *Reader) Bin() []byte {
	m := r.next()
	if r.Err != nil {
		return nil
	}
	if m < 0xc4 || m > 0xc6 {
		r.Fail("expected bin")
		return nil
	}
	return append([]byte{}, r.take(r.be(1<<(m-0xc4)))...)
}

func (r *Reader) lenHeader(fixLo, fixHi, m16, m32 byte, what string) int {
	m := r.next()
	var n uint64
	switch {
	case r.Err != nil:
		return 0
	case m >= fixLo && m <= fixHi:
		n = uint64(m & 0x0f)
	case m == m16:
		n = r.be(2)
	case m == m32:
		n = r.be(4)
	default:
		r.Fail("expected %s", what)
		return 0
	}
	if n > uint64(len(r.B)-r.Pos) { // every element takes at least one byte
		r.Fail("%s length %d exceeds payload", what, n)
		return 0
	}
	return int(n)
}

func (r *Reader) Array() int { return r.lenHeader(0x90, 0x9f, 0xdc, 0xdd, "array") }
func (r *Reader) Map() int   { return r.lenHeader(0x80, 0x8f, 0xde, 0xdf, "map") }

// Params reads the parameter array of a call and checks its arity.
func (r *Reader) Params(n int) {
	if got := r.Array(); r.Err == nil && got != n {
		r.Fail("expected %d parameters, got %d", n, got)
	}
}

// Finish checks that the whole payload was consumed.
func (r *Reader) Finish() {
	if r.Err == nil && r.Pos != len(r.B) {
		r.Fail("%d trailing bytes", len(r.B)-r.Pos)
	}
}

// Skip consumes one value of any type (unknown fields from newer peers).
func (r *Reader) Skip() { r.skip(0) }

func (r *Reader) skip(depth int) {
	if r.Err != nil {
		return
	}
	if depth > 64 || r.Pos >= len(r.B) {
		r.Fail("cannot skip")
		return
	}
	m := r.B[r.Pos]
	switch {
	case m < 0x80 || m >= 0xe0 || m == 0xc0 || m == 0xc2 || m == 0xc3:
		r.Pos++
	case m >= 0xcc && m <= 0xd3:
		r.Pos++
		r.take(1 << ((m - 0xcc) % 4))
	case m == 0xca:
		r.Pos++
		r.take(4)
	case m == 0xcb:
		r.Pos++
		r.take(8)
	case (m >= 0xa0 && m <= 0xbf) || (m >= 0xd9 && m <= 0xdb):
		r.Str()
	case m >= 0xc4 && m <= 0xc6:
		r.Bin()
	case (m >= 0x90 && m <= 0x9f) || m == 0xdc || m == 0xdd:
		n := r.Array()
		for i := 0; r.Err == nil && i < n; i++ {
			r.skip(depth + 1)
		}
	case (m >= 0x80 && m <= 0x8f) || m == 0xde || m == 0xdf:
		n := r.Map()
		for i := 0; r.Err == nil && i < 2*n; i++ {
			r.skip(depth + 1)
		}
	default:
		r.Fail("unsupported type 0x%02x", m)
	}
}

// ReadList decodes a MessagePack array of T.
func ReadList[T any](r *Reader, item func(*Reader) T) []T {
	n := r.Array()
	out := make([]T, 0, n)
	for i := 0; r.Err == nil && i < n; i++ {
		out = append(out, item(r))
	}
	return out
}

// ReadOpt decodes option<T>: nil -> nil.
func ReadOpt[T any](r *Reader, item func(*Reader) T) *T {
	if r.IsNil() {
		r.Pos++
		return nil
	}
	v := item(r)
	return &v
}

// Missing records a missing required record field.
func (r *Reader) Missing(field string) { r.Fail("missing field %s", field) }
