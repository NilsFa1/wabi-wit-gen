// SPDX-License-Identifier: EUPL-1.2

package main

// OCSP response cache of a verifier. What gemLibPki's OcspRespCache keys by
// serial number alone (a collision across CAs) is keyed here by the OCSP
// CertID: hashes of the issuer's name and key plus the serial number.
//
//   - only verified outcomes are cached (good / revoked), never errors or
//     "unknown", so a flaky responder is asked again next time;
//   - an entry lives until the response's nextUpdate, at most ocspCacheTTL;
//   - concurrent checks of the same certificate share one request;
//   - the cache is bounded (least recently used entries are evicted).

import (
	"container/list"
	"context"
	"crypto/sha256"
	"crypto/x509"
	"sync"
	"sync/atomic"
	"time"

	"github.com/gematik/zero-lab/go/gempki"
)

const (
	ocspCacheTTL     = 5 * time.Minute
	ocspCacheEntries = 10000
)

type certID [sha256.Size*2 + 32]byte

func certIDOf(cert, issuer *x509.Certificate) (id certID) {
	name := sha256.Sum256(issuer.RawSubject)
	key := sha256.Sum256(issuer.RawSubjectPublicKeyInfo)
	copy(id[:], name[:])
	copy(id[sha256.Size:], key[:])
	serial := cert.SerialNumber.Bytes()
	if len(serial) > 32 {
		serial = serial[len(serial)-32:]
	}
	copy(id[len(id)-len(serial):], serial)
	return id
}

type ocspEntry struct {
	id      certID
	result  *gempki.RevocationResult
	expires time.Time
}

type ocspCall struct {
	done   chan struct{}
	result *gempki.RevocationResult
	err    error
}

type ocspMetrics struct {
	hits   atomic.Uint64
	misses atomic.Uint64
}

type ocspCache struct {
	mu       sync.Mutex
	entries  map[certID]*list.Element // of *ocspEntry
	lru      *list.List
	inFlight map[certID]*ocspCall
	metrics  *ocspMetrics
	ttl      time.Duration
	max      int
}

func newOCSPCache(metrics *ocspMetrics) *ocspCache {
	return &ocspCache{
		metrics:  metrics,
		entries:  map[certID]*list.Element{},
		lru:      list.New(),
		inFlight: map[certID]*ocspCall{},
		ttl:      ocspCacheTTL,
		max:      ocspCacheEntries,
	}
}

// check returns the cached outcome for (cert, issuer) or runs fetch once for
// all concurrent callers and caches what it returns.
func (c *ocspCache) check(ctx context.Context, cert, issuer *x509.Certificate, now time.Time,
	fetch func(context.Context) (*gempki.RevocationResult, error)) (*gempki.RevocationResult, error) {
	id := certIDOf(cert, issuer)
	c.mu.Lock()
	if el, ok := c.entries[id]; ok {
		e := el.Value.(*ocspEntry)
		if now.Before(e.expires) {
			c.lru.MoveToFront(el)
			c.metrics.hits.Add(1)
			c.mu.Unlock()
			return e.result, nil
		}
		c.lru.Remove(el)
		delete(c.entries, id)
	}
	if call, ok := c.inFlight[id]; ok { // someone is asking the responder already
		c.metrics.hits.Add(1)
		c.mu.Unlock()
		select {
		case <-call.done:
			return call.result, call.err
		case <-ctx.Done():
			return nil, ctx.Err()
		}
	}
	call := &ocspCall{done: make(chan struct{})}
	c.inFlight[id] = call
	c.metrics.misses.Add(1)
	c.mu.Unlock()

	// The shared request must not die with the first caller's context.
	call.result, call.err = fetch(context.WithoutCancel(ctx))

	c.mu.Lock()
	delete(c.inFlight, id)
	if call.err == nil && call.result != nil &&
		(call.result.Status == gempki.RevocationStatusGood || call.result.Status == gempki.RevocationStatusRevoked) {
		expires := now.Add(c.ttl)
		if nu := call.result.NextUpdate; !nu.IsZero() && nu.Before(expires) {
			expires = nu
		}
		if expires.After(now) {
			c.entries[id] = c.lru.PushFront(&ocspEntry{id: id, result: call.result, expires: expires})
			for c.lru.Len() > c.max {
				old := c.lru.Back()
				c.lru.Remove(old)
				delete(c.entries, old.Value.(*ocspEntry).id)
			}
		}
	}
	c.mu.Unlock()
	close(call.done)
	return call.result, call.err
}
