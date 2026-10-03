// SPDX-License-Identifier: EUPL-1.2

package main

// TSL management and the OCSP cache, against a test PKI served over HTTP:
// a CA, an end-entity certificate (C.HCI.AUT), an OCSP responder the TSL
// lists, and the TSL itself with its .sha2 next to it.

import (
	"bytes"
	"context"
	"crypto"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha256"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/asn1"
	"encoding/base64"
	"fmt"
	"io"
	"math/big"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/gematik/zero-lab/go/gempki"

	"golang.org/x/crypto/ocsp"
)

type testPKI struct {
	caKey, respKey *ecdsa.PrivateKey
	ca, resp, ee   *x509.Certificate
	server         *httptest.Server

	ocspRequests atomic.Int32
	ocspDelay    time.Duration
	nextUpdate   time.Duration // of OCSP responses
	mu           sync.Mutex
	revoked      bool
	tsl          []byte // served at /tsl.xml
	sha2         string // served at /tsl.sha2 (defaults to the hash of tsl)
}

func mustKey(t *testing.T) *ecdsa.PrivateKey {
	k, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	return k
}

func mustCert(t *testing.T, tmpl, parent *x509.Certificate, pub crypto.PublicKey, key crypto.Signer) *x509.Certificate {
	der, err := x509.CreateCertificate(rand.Reader, tmpl, parent, pub, key)
	if err != nil {
		t.Fatal(err)
	}
	c, err := x509.ParseCertificate(der)
	if err != nil {
		t.Fatal(err)
	}
	return c
}

func mustOID(t *testing.T, ints ...uint64) x509.OID {
	oid, err := x509.OIDFromInts(ints)
	if err != nil {
		t.Fatal(err)
	}
	return oid
}

func newTestPKI(t *testing.T) *testPKI { return newTestPKINamed(t, "Test SMC-B CA", 1) }

func newTestPKINamed(t *testing.T, caName string, caSKI byte) *testPKI {
	p := &testPKI{caKey: mustKey(t), respKey: mustKey(t), nextUpdate: time.Hour}
	now := time.Now()
	caTmpl := &x509.Certificate{
		SerialNumber: big.NewInt(1), Subject: pkix.Name{CommonName: caName, Country: []string{"DE"}},
		NotBefore: now.Add(-time.Hour), NotAfter: now.Add(24 * time.Hour),
		KeyUsage: x509.KeyUsageCertSign | x509.KeyUsageCRLSign, BasicConstraintsValid: true, IsCA: true,
		SubjectKeyId: []byte{caSKI, 2, 3, 4},
	}
	p.ca = mustCert(t, caTmpl, caTmpl, &p.caKey.PublicKey, p.caKey) // self-signed
	p.resp = mustCert(t, &x509.Certificate{
		SerialNumber: big.NewInt(2), Subject: pkix.Name{CommonName: "Test OCSP Signer", Country: []string{"DE"}},
		NotBefore: now.Add(-time.Hour), NotAfter: now.Add(24 * time.Hour),
		KeyUsage: x509.KeyUsageDigitalSignature, ExtKeyUsage: []x509.ExtKeyUsage{x509.ExtKeyUsageOCSPSigning},
		BasicConstraintsValid: true, SubjectKeyId: []byte{5, 6, 7, 8},
	}, p.ca, &p.respKey.PublicKey, p.caKey)
	eeKey := mustKey(t)
	p.ee = mustCert(t, &x509.Certificate{
		SerialNumber: big.NewInt(1000), Subject: pkix.Name{CommonName: "Praxis Test", Country: []string{"DE"}},
		NotBefore: now.Add(-time.Hour), NotAfter: now.Add(24 * time.Hour),
		KeyUsage: x509.KeyUsageDigitalSignature, ExtKeyUsage: []x509.ExtKeyUsage{x509.ExtKeyUsageClientAuth},
		BasicConstraintsValid: true, SubjectKeyId: []byte{9, 9, 9},
		Policies: []x509.OID{mustOID(t, 1, 2, 276, 0, 76, 4, 77)},
	}, p.ca, &eeKey.PublicKey, p.caKey)

	mux := http.NewServeMux()
	mux.HandleFunc("/ocsp", p.serveOCSP)
	mux.HandleFunc("/tsl.xml", func(w http.ResponseWriter, _ *http.Request) {
		p.mu.Lock()
		defer p.mu.Unlock()
		w.Write(p.tsl)
	})
	mux.HandleFunc("/tsl.sha2", func(w http.ResponseWriter, _ *http.Request) {
		p.mu.Lock()
		defer p.mu.Unlock()
		if p.sha2 != "" {
			fmt.Fprintln(w, p.sha2)
			return
		}
		fmt.Fprintln(w, sha256Hex(p.tsl))
	})
	p.server = httptest.NewServer(mux)
	t.Cleanup(p.server.Close)
	p.tsl = p.makeTSL(1)
	return p
}

var oidCertHash = asn1.ObjectIdentifier{1, 3, 36, 8, 3, 13}

func (p *testPKI) serveOCSP(w http.ResponseWriter, r *http.Request) {
	p.ocspRequests.Add(1)
	time.Sleep(p.ocspDelay)
	body, _ := io.ReadAll(r.Body)
	req, err := ocsp.ParseRequest(body)
	if err != nil {
		http.Error(w, err.Error(), 400)
		return
	}
	hash := sha256.Sum256(p.ee.Raw)
	certHash, _ := asn1.Marshal(struct {
		Alg  pkix.AlgorithmIdentifier
		Hash []byte
	}{pkix.AlgorithmIdentifier{Algorithm: asn1.ObjectIdentifier{2, 16, 840, 1, 101, 3, 4, 2, 1}}, hash[:]})
	now := time.Now()
	tmpl := ocsp.Response{
		Status: ocsp.Good, SerialNumber: req.SerialNumber, ThisUpdate: now, NextUpdate: now.Add(p.nextUpdate),
		Certificate: p.resp, ExtraExtensions: []pkix.Extension{{Id: oidCertHash, Value: certHash}},
	}
	p.mu.Lock()
	if p.revoked {
		tmpl.Status, tmpl.RevokedAt, tmpl.RevocationReason = ocsp.Revoked, now.Add(-time.Minute), ocsp.KeyCompromise
	}
	p.mu.Unlock()
	resp, err := ocsp.CreateResponse(p.ca, p.resp, tmpl, p.respKey)
	if err != nil {
		http.Error(w, err.Error(), 500)
		return
	}
	w.Header().Set("Content-Type", "application/ocsp-response")
	w.Write(resp)
}

func (p *testPKI) makeTSL(seq int) []byte {
	b64 := func(c *x509.Certificate) string { return base64.StdEncoding.EncodeToString(c.Raw) }
	now := time.Now().UTC()
	return []byte(fmt.Sprintf(`<?xml version="1.0" encoding="UTF-8"?>
<TrustServiceStatusList xmlns="http://uri.etsi.org/02231/v2#" Id="TSL-Test-%d" TSLTag="http://uri.etsi.org/02231/TSLTag">
  <SchemeInformation>
    <TSLVersionIdentifier>3</TSLVersionIdentifier>
    <TSLSequenceNumber>%d</TSLSequenceNumber>
    <ListIssueDateTime>%s</ListIssueDateTime>
    <NextUpdate><dateTime>%s</dateTime></NextUpdate>
  </SchemeInformation>
  <TrustServiceProviderList><TrustServiceProvider><TSPServices>
    <TSPService><ServiceInformation>
      <ServiceTypeIdentifier>http://uri.etsi.org/TrstSvc/Svctype/CA/PKC</ServiceTypeIdentifier>
      <ServiceDigitalIdentity><DigitalId><X509Certificate>%s</X509Certificate></DigitalId></ServiceDigitalIdentity>
      <ServiceStatus>http://uri.etsi.org/TrstSvc/Svcstatus/inaccord</ServiceStatus>
      <StatusStartingTime>2020-01-01T00:00:00Z</StatusStartingTime>
      <ServiceSupplyPoints><ServiceSupplyPoint>%s/ocsp</ServiceSupplyPoint></ServiceSupplyPoints>
      <ServiceInformationExtensions><Extension Critical="false"><ExtensionOID>1.2.276.0.76.4.77</ExtensionOID></Extension></ServiceInformationExtensions>
    </ServiceInformation></TSPService>
    <TSPService><ServiceInformation>
      <ServiceTypeIdentifier>http://uri.etsi.org/TrstSvc/Svctype/Certstatus/OCSP</ServiceTypeIdentifier>
      <ServiceDigitalIdentity><DigitalId><X509Certificate>%s</X509Certificate></DigitalId></ServiceDigitalIdentity>
      <ServiceStatus>http://uri.etsi.org/TrstSvc/Svcstatus/inaccord</ServiceStatus>
      <StatusStartingTime>2020-01-01T00:00:00Z</StatusStartingTime>
    </ServiceInformation></TSPService>
  </TSPServices></TrustServiceProvider></TrustServiceProviderList>
</TrustServiceStatusList>`, seq, seq, now.Format(time.RFC3339), now.Add(24*time.Hour).Format(time.RFC3339),
		b64(p.ca), p.server.URL, b64(p.resp)))
}

// verifier for this PKI. x/crypto/ocsp rounds producedAt down to the minute,
// so the responses may look up to a minute old: allow that here.
func (p *testPKI) verifier() *certificateVerifier {
	v := newCertificateVerifier("Test", p.tsl).(*certificateVerifier)
	v.ocspMaxAge = 2 * time.Minute
	return v
}

func (p *testPKI) verify(t *testing.T, v CertificateVerifier, withOcsp bool) (Admission, *PkiError) {
	t.Helper()
	return v.Verify(context.Background(), p.ee.Raw, aut, withOcsp)
}

func TestOCSPGoodIsCachedPerCertificate(t *testing.T) {
	p := newTestPKI(t)
	v := p.verifier()
	for i := 0; i < 3; i++ {
		if _, e := p.verify(t, v, true); e != nil {
			t.Fatalf("%+v", *e)
		}
	}
	if n := p.ocspRequests.Load(); n != 1 {
		t.Fatalf("%d OCSP requests, want 1", n)
	}
	if s := v.CacheStats(); s.OcspMisses != 1 || s.OcspHits != 2 || s.OcspEntries != 1 {
		t.Fatalf("%+v", s)
	}
	if _, e := p.verify(t, v, false); e != nil { // without OCSP the cache is not consulted
		t.Fatalf("%+v", *e)
	}
	if s := v.CacheStats(); s.OcspHits != 2 {
		t.Fatalf("%+v", s)
	}
}

func TestOCSPConcurrentChecksShareOneRequest(t *testing.T) {
	p := newTestPKI(t)
	p.ocspDelay = 100 * time.Millisecond
	v := p.verifier()
	var wg sync.WaitGroup
	var failed atomic.Int32
	for i := 0; i < 32; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if _, e := p.verify(t, v, true); e != nil {
				failed.Add(1)
			}
		}()
	}
	wg.Wait()
	if failed.Load() != 0 || p.ocspRequests.Load() != 1 {
		t.Fatalf("%d failed, %d OCSP requests (want 0, 1)", failed.Load(), p.ocspRequests.Load())
	}
}

func TestOCSPRevokedIsReportedAndCached(t *testing.T) {
	p := newTestPKI(t)
	p.revoked = true
	v := p.verifier()
	for i := 0; i < 2; i++ {
		_, e := p.verify(t, v, true)
		if e == nil || e.VerificationFailed.Code != "SW_1047_CERT_REVOKED" {
			t.Fatalf("%+v", e)
		}
	}
	if n := p.ocspRequests.Load(); n != 1 {
		t.Fatalf("%d OCSP requests, want 1", n)
	}
}

func TestOCSPEntryExpiresAtNextUpdate(t *testing.T) {
	p := newTestPKI(t)
	p.nextUpdate = 300 * time.Millisecond
	v := p.verifier()
	p.verify(t, v, true)
	time.Sleep(400 * time.Millisecond)
	if _, e := p.verify(t, v, true); e != nil {
		t.Fatalf("%+v", *e)
	}
	if n := p.ocspRequests.Load(); n != 2 {
		t.Fatalf("%d OCSP requests, want 2 (entry expired at nextUpdate)", n)
	}
}

func TestOCSPUnavailableIsNotCached(t *testing.T) {
	p := newTestPKI(t)
	v := p.verifier()
	p.server.Close()
	for i := 0; i < 2; i++ {
		_, e := p.verify(t, v, true)
		if e == nil || e.VerificationFailed.Code != "TE_1032_OCSP_NOT_AVAILABLE" {
			t.Fatalf("%+v", e)
		}
	}
	if s := v.CacheStats(); s.OcspMisses != 2 || s.OcspEntries != 0 {
		t.Fatalf("%+v", s)
	}
}

func TestTslInfo(t *testing.T) {
	p := newTestPKI(t)
	info, e := p.verifier().TslInfo()
	if e != nil {
		t.Fatalf("%+v", *e)
	}
	if info.Id != "TSL-Test-1" || info.SequenceNumber != 1 || info.Sha256 != sha256Hex(p.tsl) ||
		info.IssueDate == "" || info.NextUpdate == "" || info.Expired {
		t.Fatalf("%+v", info)
	}
}

func TestSetTslSwapsAndRejectsGarbage(t *testing.T) {
	p := newTestPKI(t)
	other := newTestPKINamed(t, "Other CA", 2)
	v := p.verifier()
	if _, e := p.verify(t, v, false); e != nil {
		t.Fatalf("%+v", *e)
	}
	if info, e := v.SetTsl(other.makeTSL(7)); e != nil || info.SequenceNumber != 7 {
		t.Fatalf("%+v %+v", info, e)
	}
	if _, e := p.verify(t, v, false); e == nil || e.VerificationFailed.Code != "TE_1027_CA_CERT_MISSING" {
		t.Fatalf("certificate of the old CA after the swap: %+v", e)
	}
	if _, e := v.SetTsl([]byte("<garbage/>")); e == nil || e.Tag != PkiErrorInvalidInput {
		t.Fatalf("%+v", e)
	}
	if info, _ := v.TslInfo(); info.SequenceNumber != 7 {
		t.Fatalf("a rejected TSL must not replace the current one: %+v", info)
	}
}

func TestRefreshTsl(t *testing.T) {
	p := newTestPKI(t)
	v := p.verifier()
	url := p.server.URL + "/tsl.xml"

	if got, e := v.RefreshTsl(context.Background(), url); e != nil || got != nil {
		t.Fatalf("unchanged TSL: %v %+v", got, e)
	}
	p.mu.Lock()
	p.tsl = p.makeTSL(2)
	p.mu.Unlock()
	got, e := v.RefreshTsl(context.Background(), url)
	if e != nil || got == nil || !bytes.Equal(*got, p.tsl) {
		t.Fatalf("changed TSL: %+v", e)
	}
	if info, _ := v.TslInfo(); info.SequenceNumber != 2 {
		t.Fatalf("%+v", info)
	}

	p.mu.Lock()
	p.tsl, p.sha2 = p.makeTSL(3), strings.Repeat("ab", 32) // announced hash does not match
	p.mu.Unlock()
	if _, e := v.RefreshTsl(context.Background(), url); e == nil || e.Tag != PkiErrorDownloadFailed {
		t.Fatalf("hash mismatch: %+v", e)
	}
	if info, _ := v.TslInfo(); info.SequenceNumber != 2 {
		t.Fatalf("a failed refresh must keep the TSL: %+v", info)
	}
	if _, e := v.RefreshTsl(context.Background(), p.server.URL+"/tsl"); e == nil || e.Tag != PkiErrorDownloadFailed {
		t.Fatalf("URL without .xml: %+v", e)
	}
}

func TestSetTslInvalidatesOCSPAndPreservesCounters(t *testing.T) {
	p := newTestPKI(t)
	v := p.verifier()
	for i := 0; i < 2; i++ {
		if _, e := p.verify(t, v, true); e != nil {
			t.Fatalf("initial verification: %+v", e)
		}
	}
	if _, e := v.SetTsl(p.makeTSL(2)); e != nil {
		t.Fatal(e)
	}
	if stats := v.CacheStats(); stats.OcspEntries != 0 || stats.OcspHits != 1 || stats.OcspMisses != 1 {
		t.Fatalf("new generation must retain counts, not entries: %+v", stats)
	}
	p.mu.Lock()
	p.revoked = true
	p.mu.Unlock()
	if _, e := p.verify(t, v, true); e == nil || e.VerificationFailed.Code != "SW_1047_CERT_REVOKED" {
		t.Fatalf("old good response survived TSL change: %+v", e)
	}
	if p.ocspRequests.Load() != 2 {
		t.Fatalf("new generation did not fetch: %d", p.ocspRequests.Load())
	}
	if stats := v.CacheStats(); stats.OcspHits != 1 || stats.OcspMisses != 2 {
		t.Fatalf("counts must span generations: %+v", stats)
	}
}

func TestOCSPRequestsInFlightStayWithOldVerifierGeneration(t *testing.T) {
	p := newTestPKI(t)
	v := p.verifier()
	old, err := v.current()
	if err != nil {
		t.Fatal(err)
	}
	entered, resume, done := make(chan struct{}), make(chan struct{}), make(chan error, 1)
	go func() {
		_, err := old.ocsp.check(context.Background(), p.ee, p.ca, time.Now(),
			func(context.Context) (*gempki.RevocationResult, error) {
				close(entered)
				<-resume
				return &gempki.RevocationResult{Status: gempki.RevocationStatusGood, NextUpdate: time.Now().Add(time.Minute)}, nil
			})
		done <- err
	}()
	<-entered // the old request is definitely in flight
	_, e := v.SetTsl(p.makeTSL(2))
	close(resume)
	if err := <-done; err != nil {
		t.Fatal(err)
	}
	if e != nil {
		t.Fatal(e)
	}
	current, err := v.current()
	if err != nil {
		t.Fatal(err)
	}
	if current.ocsp == old.ocsp || current == old {
		t.Fatal("TSL generations share a cache")
	}
	if stats := v.CacheStats(); stats.OcspEntries != 0 || stats.OcspMisses != 1 {
		t.Fatalf("late old response populated the new cache: %+v", stats)
	}
	// Identical parsing snapshots may be shared, HTTP results may not.
	other := p.verifier()
	second, err := other.current()
	if err != nil {
		t.Fatal(err)
	}
	if second.tsl != old.tsl || second.ocsp == old.ocsp {
		t.Fatal("parsed snapshots should be shared, OCSP caches must be private")
	}
}
