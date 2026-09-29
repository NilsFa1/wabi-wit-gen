package main

// Payload encoding v1 (ABI.md §5) — the same MessagePack subset every SDK
// runtime and every core implements. Independent Go implementation.

import (
	"encoding/binary"
	"errors"
	"math"
	"unicode/utf8"
)

type writer struct{ b []byte }

func (w *writer) nil_()        { w.b = append(w.b, 0xc0) }
func (w *writer) bool_(v bool) { w.b = append(w.b, map[bool]byte{false: 0xc2, true: 0xc3}[v]) }
func (w *writer) uint(v uint64) {
	switch {
	case v < 0x80:
		w.b = append(w.b, byte(v))
	case v <= math.MaxUint8:
		w.b = append(w.b, 0xcc, byte(v))
	case v <= math.MaxUint16:
		w.b = binary.BigEndian.AppendUint16(append(w.b, 0xcd), uint16(v))
	case v <= math.MaxUint32:
		w.b = binary.BigEndian.AppendUint32(append(w.b, 0xce), uint32(v))
	default:
		w.b = binary.BigEndian.AppendUint64(append(w.b, 0xcf), v)
	}
}
func (w *writer) header(n int, fixBase byte, fixMax int, m8 byte, m16, m32 byte) {
	switch {
	case fixMax > 0 && n <= fixMax:
		w.b = append(w.b, fixBase|byte(n))
	case m8 != 0 && n <= math.MaxUint8:
		w.b = append(w.b, m8, byte(n))
	case n <= math.MaxUint16:
		w.b = binary.BigEndian.AppendUint16(append(w.b, m16), uint16(n))
	default:
		w.b = binary.BigEndian.AppendUint32(append(w.b, m32), uint32(n))
	}
}
func (w *writer) str(s string) { w.header(len(s), 0xa0, 31, 0xd9, 0xda, 0xdb); w.b = append(w.b, s...) }
func (w *writer) bin(v []byte) { w.header(len(v), 0, 0, 0xc4, 0xc5, 0xc6); w.b = append(w.b, v...) }
func (w *writer) array(n int)  { w.header(n, 0x90, 15, 0, 0xdc, 0xdd) }
func (w *writer) map_(n int)   { w.header(n, 0x80, 15, 0, 0xde, 0xdf) }

var errPayload = errors.New("malformed payload")

type reader struct {
	b   []byte
	pos int
}

func (r *reader) byte_() (byte, error) {
	if r.pos >= len(r.b) {
		return 0, errPayload
	}
	r.pos++
	return r.b[r.pos-1], nil
}
func (r *reader) take(n uint64) ([]byte, error) {
	if n > uint64(len(r.b)-r.pos) {
		return nil, errPayload
	}
	s := r.b[r.pos : r.pos+int(n)]
	r.pos += int(n)
	return s, nil
}
func (r *reader) be(n int) (uint64, error) {
	s, err := r.take(uint64(n))
	var v uint64
	for _, c := range s {
		v = v<<8 | uint64(c)
	}
	return v, err
}
func (r *reader) isNil() bool { return r.pos < len(r.b) && r.b[r.pos] == 0xc0 }
func (r *reader) uint() (uint64, error) {
	m, err := r.byte_()
	switch {
	case err != nil:
		return 0, err
	case m < 0x80:
		return uint64(m), nil
	case m >= 0xcc && m <= 0xcf:
		return r.be(1 << (m - 0xcc))
	}
	return 0, errPayload
}
func (r *reader) str() (string, error) {
	m, err := r.byte_()
	if err != nil {
		return "", err
	}
	var n uint64
	switch {
	case m >= 0xa0 && m <= 0xbf:
		n = uint64(m & 0x1f)
	case m >= 0xd9 && m <= 0xdb:
		if n, err = r.be(1 << (m - 0xd9)); err != nil {
			return "", err
		}
	default:
		return "", errPayload
	}
	s, err := r.take(n)
	if err != nil || !utf8.Valid(s) {
		return "", errPayload
	}
	return string(s), nil
}
func (r *reader) bin() ([]byte, error) {
	m, err := r.byte_()
	if err != nil || m < 0xc4 || m > 0xc6 {
		return nil, errPayload
	}
	n, err := r.be(1 << (m - 0xc4))
	if err != nil {
		return nil, err
	}
	s, err := r.take(n)
	return append([]byte(nil), s...), err
}
func (r *reader) lenHeader(fixLo, fixHi, m16, m32 byte) (int, error) {
	m, err := r.byte_()
	switch {
	case err != nil:
		return 0, err
	case m >= fixLo && m <= fixHi:
		return int(m & 0x0f), nil
	case m == m16:
		n, err := r.be(2)
		return int(n), err
	case m == m32:
		n, err := r.be(4)
		if n > uint64(len(r.b)) {
			return 0, errPayload
		}
		return int(n), err
	}
	return 0, errPayload
}
func (r *reader) array() (int, error) { return r.lenHeader(0x90, 0x9f, 0xdc, 0xdd) }
func (r *reader) map_() (int, error)  { return r.lenHeader(0x80, 0x8f, 0xde, 0xdf) }
func (r *reader) optStr() (*string, error) {
	if r.isNil() {
		r.pos++
		return nil, nil
	}
	s, err := r.str()
	return &s, err
}
func (r *reader) skip(depth int) error {
	if depth > 64 || r.pos >= len(r.b) {
		return errPayload
	}
	m := r.b[r.pos]
	switch {
	case m < 0x80 || m >= 0xe0 || m == 0xc0 || m == 0xc2 || m == 0xc3:
		r.pos++
	case m >= 0xcc && m <= 0xd3:
		r.pos++
		_, err := r.take(1 << ((m - 0xcc) % 4))
		return err
	case m == 0xca || m == 0xcb:
		r.pos++
		_, err := r.take(map[byte]uint64{0xca: 4, 0xcb: 8}[m])
		return err
	case (m >= 0xa0 && m <= 0xbf) || (m >= 0xd9 && m <= 0xdb):
		_, err := r.str()
		return err
	case m >= 0xc4 && m <= 0xc6:
		_, err := r.bin()
		return err
	case (m >= 0x90 && m <= 0x9f) || m == 0xdc || m == 0xdd:
		n, err := r.array()
		for i := 0; err == nil && i < n; i++ {
			err = r.skip(depth + 1)
		}
		return err
	case (m >= 0x80 && m <= 0x8f) || m == 0xde || m == 0xdf:
		n, err := r.map_()
		for i := 0; err == nil && i < 2*n; i++ {
			err = r.skip(depth + 1)
		}
		return err
	default:
		return errPayload
	}
	return nil
}
