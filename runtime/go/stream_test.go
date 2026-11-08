package wabiabi

import (
	"bytes"
	"testing"
	"time"
)

func takePull(t *testing.T, h uint64) []byte {
	t.Helper()
	defer FutureDrop(h)
	deadline := time.Now().Add(2 * time.Second)
	for FuturePoll(h) == Pending {
		if time.Now().After(deadline) {
			t.Fatal("pull did not complete")
		}
		time.Sleep(time.Millisecond)
	}
	p, n, status := FutureTake(h)
	defer Free(p)
	if status != OK {
		t.Fatalf("take: %d", status)
	}
	b, ok := Borrow(p, n)
	if !ok {
		t.Fatal("bad payload")
	}
	return bytes.Clone(b)
}

// Force cancellation after dequeue, before delivery: no probabilistic sleeps.
func TestCancelledDequeuePreservesOrder(t *testing.T) {
	items := make(chan []byte, 1)
	items <- []byte("second")
	s := &stream{items: items, stop: make(chan struct{}), release: func() {}}
	f, _ := newFuture()
	defer FutureDrop(f.handle)
	f.deferredCancel = true
	s.busy.Store(true)
	f.cancelByHost()
	if FuturePoll(f.handle) != Pending {
		t.Fatal("cancellation became visible before stream cleanup")
	}
	s.finishPull(f, OK, []byte("first"))
	if FuturePoll(f.handle) != Ready || s.busy.Load() {
		t.Fatal("completion must publish only after releasing busy")
	}
	f.mu.Lock()
	status := f.status
	f.mu.Unlock()
	if status != ErrCancelled {
		t.Fatalf("cancel lost: %d", status)
	}
	for _, want := range []string{"first", "second"} {
		h, st := s.next()
		if st != OK {
			t.Fatalf("next: %d", st)
		}
		if got := takePull(t, h); !bytes.Equal(got, []byte(want)) {
			t.Fatalf("got %q, want %q", got, want)
		}
	}
}

func TestPullCanRestartImmediatelyAfterCompletion(t *testing.T) {
	s := &stream{items: make(chan []byte), stop: make(chan struct{}), release: func() {}}
	for _, cancelled := range []bool{false, true} {
		f, _ := newFuture()
		f.deferredCancel = true
		s.busy.Store(true)
		if cancelled {
			f.cancelByHost()
		}
		s.finishPull(f, OK, []byte("item"))
		h, st := s.next()
		if st != OK {
			t.Fatalf("immediate next after cancellation=%v: %d", cancelled, st)
		}
		FutureCancel(h)
		deadline := time.Now().Add(2 * time.Second)
		for FuturePoll(h) == Pending && time.Now().Before(deadline) {
			time.Sleep(time.Millisecond)
		}
		if FuturePoll(h) != Ready {
			t.Fatal("cancelled pull did not release busy")
		}
		FutureDrop(h)
		FutureDrop(f.handle)
	}
}

func TestStreamCancelCancelsPendingPullButEndsLaterPulls(t *testing.T) {
	for i := 0; i < 32; i++ {
		s := &stream{items: make(chan []byte), stop: make(chan struct{}), release: func() {}}
		h, status := s.next()
		if status != OK {
			t.Fatalf("next: %d", status)
		}
		s.cancel()
		deadline := time.Now().Add(2 * time.Second)
		for FuturePoll(h) == Pending && time.Now().Before(deadline) {
			time.Sleep(time.Millisecond)
		}
		p, _, status := FutureTake(h)
		Free(p)
		FutureDrop(h)
		if status != ErrCancelled {
			t.Fatalf("pull issued before cancellation: %d", status)
		}
		h, status = s.next()
		if status != OK {
			t.Fatalf("later next: %d", status)
		}
		p, _, status = FutureTake(h)
		Free(p)
		FutureDrop(h)
		if status != StreamEnd {
			t.Fatalf("pull issued after cancellation: %d", status)
		}
	}
}
