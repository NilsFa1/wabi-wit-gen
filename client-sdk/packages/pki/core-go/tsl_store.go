// SPDX-License-Identifier: EUPL-1.2

package main

// The TSL a verifier works with: an immutable snapshot (raw bytes, hash,
// parsed CA and OCSP responder list), swapped atomically by set-tsl and
// refresh-tsl so that verifications in flight keep the snapshot they began
// with. Snapshots of identical TSL bytes are shared between verifiers.
//
// refresh-tsl follows gematik's distribution: next to TSL.xml lies TSL.sha2
// (hex SHA-256 of the XML). Only if that differs from the TSL in use is the
// XML downloaded, checked against the announced hash and parsed.
//
// Not yet: the TSL signature (TUC_PKI_001, XAdES), TSL ID/sequence number
// monotonicity; NextUpdate is reported (tsl-info.expired), not enforced.

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/gematik/zero-lab/go/gempki/tsl"
)

type tslSnapshot struct {
	raw   []byte
	hash  string // hex SHA-256 of raw
	trust *trustList
	info  TslInfo // Expired is computed on request
	next  time.Time
}

// snapshotCache shares parsed TSLs between verifiers (keyed by hash; a
// handful of recent ones, TSLs are several hundred kilobytes).
var snapshotCache = struct {
	sync.Mutex
	byHash map[string]*tslSnapshot
	order  []string
}{byHash: map[string]*tslSnapshot{}}

const snapshotCacheSize = 4

func sha256Hex(b []byte) string {
	sum := sha256.Sum256(b)
	return hex.EncodeToString(sum[:])
}

func loadSnapshot(raw []byte) (*tslSnapshot, error) {
	hash := sha256Hex(raw)
	snapshotCache.Lock()
	if s, ok := snapshotCache.byHash[hash]; ok {
		snapshotCache.Unlock()
		return s, nil
	}
	snapshotCache.Unlock()

	list, err := tsl.Parse(bytes.NewReader(raw), "")
	if err != nil {
		return nil, fmt.Errorf("TSL could not be parsed: %v", err)
	}
	trust, err := trustListOf(list)
	if err != nil {
		return nil, err
	}
	s := &tslSnapshot{raw: bytes.Clone(raw), hash: hash, trust: trust}
	si := list.SchemeInformation
	s.info = TslInfo{Id: list.Id, SequenceNumber: si.TSLSequenceNumber, Sha256: hash}
	if t := time.Time(si.ListIssueDateTime); !t.IsZero() {
		s.info.IssueDate = t.UTC().Format(time.RFC3339)
	}
	if t := time.Time(si.NextUpdate); !t.IsZero() {
		s.next = t
		s.info.NextUpdate = t.UTC().Format(time.RFC3339)
	}

	snapshotCache.Lock()
	defer snapshotCache.Unlock()
	if prev, ok := snapshotCache.byHash[hash]; ok { // parsed concurrently
		return prev, nil
	}
	snapshotCache.byHash[hash] = s
	snapshotCache.order = append(snapshotCache.order, hash)
	if len(snapshotCache.order) > snapshotCacheSize {
		delete(snapshotCache.byHash, snapshotCache.order[0])
		snapshotCache.order = snapshotCache.order[1:]
	}
	return s, nil
}

func (s *tslSnapshot) infoAt(now time.Time) TslInfo {
	info := s.info
	info.Expired = !s.next.IsZero() && now.After(s.next)
	return info
}

// maxTSLSize bounds a download (the TI TSL is well below this).
const maxTSLSize = 32 << 20

func download(ctx context.Context, client *http.Client, url string, limit int64) ([]byte, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, err
	}
	resp, err := client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("GET %s: %s", url, resp.Status)
	}
	b, err := io.ReadAll(io.LimitReader(resp.Body, limit+1))
	if err != nil {
		return nil, fmt.Errorf("GET %s: %v", url, err)
	}
	if int64(len(b)) > limit {
		return nil, fmt.Errorf("GET %s: larger than %d bytes", url, limit)
	}
	return b, nil
}

// sha2URL is where gematik publishes the hash of a TSL: TSL.xml -> TSL.sha2.
func sha2URL(tslURL string) (string, error) {
	if !strings.HasSuffix(strings.ToLower(tslURL), ".xml") {
		return "", fmt.Errorf("TSL URL must end in .xml: %q", tslURL)
	}
	return tslURL[:len(tslURL)-4] + ".sha2", nil
}

// fetchIfChanged returns the new TSL bytes, or nil if the published hash equals current.
func fetchIfChanged(ctx context.Context, client *http.Client, url, current string) ([]byte, error) {
	hashURL, err := sha2URL(url)
	if err != nil {
		return nil, err
	}
	hb, err := download(ctx, client, hashURL, 1024)
	if err != nil {
		return nil, err
	}
	announced := strings.ToLower(strings.TrimSpace(string(hb)))
	if announced == current {
		return nil, nil
	}
	raw, err := download(ctx, client, url, maxTSLSize)
	if err != nil {
		return nil, err
	}
	if got := sha256Hex(raw); got != announced {
		return nil, fmt.Errorf("downloaded TSL has SHA-256 %s, %s announces %s", got, hashURL, announced)
	}
	return raw, nil
}
