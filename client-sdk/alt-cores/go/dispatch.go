package main

// Method-id dispatch and payload (de)serialisation for the Go core.
// In the Rust core this file is *generated* (core-rust/src/generated); a Go
// emitter for acme-wit-gen would produce the equivalent. It is hand-written
// here to keep the stub small — it is core-side code, the SDKs never see it.

import "context"

func encodeCustomer(w *writer, c customer) {
	n := 2
	if c.email != nil {
		n++
	}
	w.map_(n)
	w.uint(1)
	w.str(c.id)
	w.uint(2)
	w.str(c.name)
	if c.email != nil {
		w.uint(3)
		w.str(*c.email)
	}
}

func encodeError(w *writer, e *clientError) {
	if e.tag >= 4 {
		w.array(2)
		w.uint(e.tag)
		w.str(e.msg)
	} else {
		w.array(1)
		w.uint(e.tag)
	}
}

// result<T, client-error> -> [0, T] | [1, E]
func result(err *clientError, ok func(w *writer)) []byte {
	w := &writer{}
	w.array(2)
	if err != nil {
		w.uint(1)
		encodeError(w, err)
	} else {
		w.uint(0)
		ok(w)
	}
	return w.b
}

func encodeEvent(e event) []byte {
	w := &writer{}
	w.array(2)
	w.uint(e.tag)
	switch e.tag {
	case 1:
		encodeCustomer(w, e.customer)
	case 2:
		w.uint(uint64(e.state))
	case 3:
		w.str(e.text)
	}
	return w.b
}

func params(req []byte, n int) (*reader, bool) {
	r := &reader{b: req}
	got, err := r.array()
	return r, err == nil && got == n
}

func dispatchSync(object uint64, method uint32, req []byte) ([]byte, int32) {
	switch method {
	case mCtor:
		if object != 0 {
			return nil, errInvalidHandle
		}
		r, ok := params(req, 1)
		if !ok {
			return []byte("expected 1 parameter"), errInvalidArgument
		}
		n, err := r.map_()
		if err != nil {
			return []byte("client-options must be a map"), errInvalidArgument
		}
		var o options
		have := map[uint64]bool{}
		for i := 0; i < n && err == nil; i++ {
			var tag uint64
			if tag, err = r.uint(); err != nil {
				break
			}
			have[tag] = true
			switch tag {
			case 1:
				o.endpoint, err = r.str()
			case 2:
				o.apiKey, err = r.optStr()
			case 3:
				o.timeoutMs, err = r.uint()
			default:
				err = r.skip(0) // unknown field from a newer SDK
			}
		}
		if err != nil || !have[1] || !have[3] || r.pos != len(r.b) {
			return []byte("malformed client-options"), errInvalidArgument
		}
		w := &writer{}
		w.uint(newHandle(kindObject, newClient(o)))
		return w.b, statusOK
	case mState, mEvents:
		c, ok := lookup[*client](object)
		if !ok {
			return nil, errInvalidHandle
		}
		if _, ok := params(req, 0); !ok {
			return []byte("expected 0 parameters"), errInvalidArgument
		}
		w := &writer{}
		if method == mState {
			w.uint(uint64(c.getState()))
			return w.b, statusOK
		}
		events, unsubscribe := c.subscribe()
		items := make(chan []byte)
		s := &stream{items: items, stop: make(chan struct{}), release: unsubscribe}
		go func() { // encode lazily, stop when the stream is cancelled
			defer close(items)
			for {
				select {
				case e, ok := <-events:
					if !ok {
						return
					}
					select {
					case items <- encodeEvent(e):
					case <-s.stop:
						return
					}
				case <-s.stop:
					return
				}
			}
		}()
		w.uint(newHandle(kindStream, s))
		return w.b, statusOK
	case mConnect, mDisconnect, mGetCustomer, mCreateCustomer, mSendCommand:
		if _, ok := lookup[*client](object); !ok {
			return nil, errInvalidHandle
		}
		return nil, errUnsupported
	}
	return nil, errUnknownMethod
}

func dispatchAsync(object uint64, method uint32, req []byte) (func(context.Context) []byte, int32) {
	if method == mCtor || method == mState || method == mEvents {
		return nil, errUnsupported
	}
	if method < mConnect || method > mSendCommand {
		return nil, errUnknownMethod
	}
	c, ok := lookup[*client](object)
	if !ok {
		return nil, errInvalidHandle
	}
	unit := func(w *writer) { w.nil_() }
	switch method {
	case mConnect, mDisconnect:
		if _, ok := params(req, 0); !ok {
			return nil, errInvalidArgument
		}
		return func(ctx context.Context) []byte {
			if method == mConnect {
				return result(c.connect(ctx), unit)
			}
			return result(c.disconnect(), unit)
		}, statusOK
	case mGetCustomer:
		r, ok := params(req, 1)
		id, err := r.str()
		if !ok || err != nil {
			return nil, errInvalidArgument
		}
		return func(context.Context) []byte {
			cu, e := c.getCustomer(id)
			return result(e, func(w *writer) { encodeCustomer(w, cu) })
		}, statusOK
	case mCreateCustomer:
		r, ok := params(req, 2)
		name, err1 := r.str()
		email, err2 := r.optStr()
		if !ok || err1 != nil || err2 != nil {
			return nil, errInvalidArgument
		}
		return func(context.Context) []byte {
			cu, e := c.createCustomer(name, email)
			return result(e, func(w *writer) { encodeCustomer(w, cu) })
		}, statusOK
	default: // mSendCommand
		r, ok := params(req, 1)
		n, err := r.map_()
		if !ok || err != nil {
			return nil, errInvalidArgument
		}
		var name string
		var payload []byte
		for i := 0; i < n && err == nil; i++ {
			var tag uint64
			if tag, err = r.uint(); err != nil {
				break
			}
			switch tag {
			case 1:
				name, err = r.str()
			case 2:
				payload, err = r.bin()
			default:
				err = r.skip(0)
			}
		}
		if err != nil {
			return nil, errInvalidArgument
		}
		return func(ctx context.Context) []byte {
			out, e := c.sendCommand(ctx, name, payload)
			return result(e, func(w *writer) {
				w.map_(2)
				w.uint(1)
				w.str(name)
				w.uint(2)
				w.bin(out)
			})
		}, statusOK
	}
}
