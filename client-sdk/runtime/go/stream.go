package wabiabi

import (
	"sync"
	"sync/atomic"
)

// Stream is what a core returns for a WIT `stream<T>`: items arrive on C
// until it is closed (end of stream); Cancel is called once when the host
// cancels or drops the stream and must make the producer stop.
type Stream[T any] struct {
	C      <-chan T
	Cancel func()
}

// stream is the generic pull stream on top of futures (ABI.md §7).
type stream struct {
	mu       sync.Mutex
	items    <-chan []byte
	leftover [][]byte // items whose pull was cancelled after dequeue: delivered first
	busy     atomic.Bool
	stop     chan struct{}
	stopOnce sync.Once
	release  func()
}

// NewStream registers a typed stream; items are encoded lazily with enc.
func NewStream[T any](s Stream[T], enc func(*Writer, T)) uint64 {
	items := make(chan []byte)
	st := &stream{items: items, stop: make(chan struct{}), release: func() {
		if s.Cancel != nil {
			s.Cancel()
		}
	}}
	go func() {
		defer close(items)
		for {
			select {
			case it, ok := <-s.C:
				if !ok {
					return
				}
				w := &Writer{}
				enc(w, it)
				select {
				case items <- w.B:
				case <-st.stop:
					return
				}
			case <-st.stop:
				return
			}
		}
	}()
	return newHandle(KindStream, st)
}

func (s *stream) next() (uint64, int32) {
	if !s.busy.CompareAndSwap(false, true) {
		return 0, ErrBusy
	}
	f, ctx := newFuture()
	f.deferredCancel = true // set before the handle is returned to the host
	if s.stopped() {
		s.finishPull(f, StreamEnd, nil)
		return f.handle, OK
	}
	go func() {
		var status int32
		var item []byte
		if s.stopped() {
			// This pull was issued before stream cancellation, even if its
			// worker starts afterwards. Only later pulls report stream end.
			status = ErrCancelled
		} else {
			s.mu.Lock()
			if len(s.leftover) > 0 {
				item = s.leftover[0]
				s.leftover = s.leftover[1:]
				status = OK
				s.mu.Unlock()
			} else {
				s.mu.Unlock()
				select {
				case it, ok := <-s.items:
					switch {
					case ok:
						status, item = OK, it
					case s.stopped():
						status = ErrCancelled
					default:
						status = StreamEnd
					}
				case <-s.stop:
					status = ErrCancelled
				case <-ctx.Done():
					status = ErrCancelled
				}
			}
		}
		s.finishPull(f, status, item)
	}()
	return f.handle, OK
}

// Finalize the winning outcome and any leftover before notifying the host.
// A callback (including cancellation) may immediately start the next pull.
func (s *stream) finishPull(f *future, status int32, item []byte) {
	f.completeWith(status, item, func(accepted bool) {
		if !accepted && status == OK {
			s.mu.Lock()
			s.leftover = append([][]byte{item}, s.leftover...)
			s.mu.Unlock()
		}
		s.busy.Store(false)
	})
}

func (s *stream) stopped() bool {
	select {
	case <-s.stop:
		return true
	default:
		return false
	}
}

func (s *stream) cancel() {
	s.stopOnce.Do(func() {
		close(s.stop)
		s.release()
	})
}
