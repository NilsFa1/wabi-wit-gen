// SPDX-License-Identifier: EUPL-1.2

package main

// The gematik:pki core in Go: TUC_PKI_018 certificate verification in the
// classic TI (1.0) trust model — the TSL is the trust source — built on
// gematik zero-lab's gempki (certificate and TSL parsing with Brainpool
// support, admission extension, OCSP). The checks themselves follow the Java
// reference gemLibPki 5.0.2 step by step (tuc_pki_018.go), with its error
// codes, so both cores answer the same inputs the same way.

import (
	"bytes"
	"context"
	"crypto/x509"
	"fmt"
	"net/http"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/gematik/zero-lab/go/gempki"
	"github.com/gematik/zero-lab/go/gempki/tsl"
)

const coreInfo = "gematik-pki-go-core/0.1.0 (gempki v0.23.0, TUC_PKI_018 after gemLibPki 5.0.2)"

const svcStatusRevoked = "http://uri.etsi.org/TrstSvc/Svcstatus/revoked"

// caService is one CA/PKC service of the TSL: a CA certificate with the
// status and the certificate types (ExtensionOIDs) the TSL grants it.
type caService struct {
	cert         *x509.Certificate
	status       string
	statusStart  time.Time
	typeOIDs     map[string]bool
	supplyPoints []string
}

// trustList is the part of a TSL TUC_PKI_018 needs. It is immutable once
// built and shared by all verifications of a verifier.
type trustList struct {
	cas        []*caService
	responders []*x509.Certificate // OCSP signers listed in the TSL
}

// trustListOf extracts the CA/PKC services and OCSP responders of a parsed TSL.
func trustListOf(list *tsl.List) (*trustList, error) {
	if len(list.TrustServiceProviderList) == 0 {
		return nil, fmt.Errorf("TSL lists no trust service providers")
	}
	t := &trustList{}
	for _, r := range tsl.OCSPResponders(list) {
		t.responders = append(t.responders, r.Cert)
	}
	for i := range list.TrustServiceProviderList {
		for j := range list.TrustServiceProviderList[i].TSPServices {
			info := &list.TrustServiceProviderList[i].TSPServices[j].ServiceInformation
			cert := info.ServiceDigitalIdentity.DigitalId.X509Certificate
			if info.ServiceTypeIdentifier != tsl.ServiceTypeCaPkc || cert == nil {
				continue
			}
			svc := &caService{
				cert:         cert,
				status:       strings.TrimSpace(info.ServiceStatus),
				statusStart:  time.Time(info.StatusStartingTime),
				typeOIDs:     map[string]bool{},
				supplyPoints: info.ServiceSupplyPoints,
			}
			for _, ext := range info.ServiceInformationExtensions {
				if oid := strings.TrimSpace(ext.ExtensionOID); oid != "" {
					svc.typeOIDs[oid] = true
				}
			}
			t.cas = append(t.cas, svc)
		}
	}
	return t, nil
}

type certificateVerifier struct {
	productType string
	http        *http.Client
	nowFunc     func() time.Time
	ocsp        *ocspCache
	ocspMaxAge  time.Duration // producedAt window; 0 = gempki's default (37.5 s, as TUC_PKI_006)

	mu       sync.Mutex // serializes TSL changes; readers use the atomic pointer
	initial  []byte     // constructor TSL, parsed on first use (the constructor cannot fail)
	snapshot atomic.Pointer[tslSnapshot]
}

// newCertificateVerifier is the constructor the generated dispatcher calls.
func newCertificateVerifier(productType string, tslXML []byte) CertificateVerifier {
	return &certificateVerifier{
		productType: productType,
		initial:     bytes.Clone(tslXML),
		http:        &http.Client{Timeout: 10 * time.Second},
		nowFunc:     time.Now,
		ocsp:        newOCSPCache(),
	}
}

func (v *certificateVerifier) Close() {}

// current returns the TSL in use, parsing the constructor's TSL on first use.
func (v *certificateVerifier) current() (*tslSnapshot, error) {
	if s := v.snapshot.Load(); s != nil {
		return s, nil
	}
	v.mu.Lock()
	defer v.mu.Unlock()
	if s := v.snapshot.Load(); s != nil {
		return s, nil
	}
	s, err := loadSnapshot(v.initial)
	if err != nil {
		return nil, err
	}
	v.snapshot.Store(s)
	return s, nil
}

func invalidInput(m string) *PkiError { return &PkiError{Tag: PkiErrorInvalidInput, InvalidInput: m} }
func downloadFailed(m string) *PkiError {
	return &PkiError{Tag: PkiErrorDownloadFailed, DownloadFailed: m}
}

func (v *certificateVerifier) Verify(ctx context.Context, der []byte, profiles []CertificateProfile, withOcsp bool) (Admission, *PkiError) {
	if len(profiles) == 0 {
		return Admission{}, invalidInput("at least one certificate profile is required")
	}
	snap, err := v.current()
	if err != nil {
		return Admission{}, invalidInput(err.Error())
	}
	cert, err := gempki.ParseCertificate(der)
	if err != nil {
		return Admission{}, invalidInput("certificate is not a DER-encoded X.509 certificate: " + err.Error())
	}
	c := &tucPki018{productType: v.productType, trust: snap.trust, now: v.nowFunc().UTC()}
	if withOcsp {
		c.ocsp = &gempki.OCSPChecker{HTTPClient: v.http, TSLResponders: snap.trust.responders, RequireCertHash: true,
			MaxResponseAge: v.ocspMaxAge}
		c.ocspCache = v.ocsp
	}
	profile, failure := c.verify(ctx, cert, profiles)
	if failure != nil {
		return Admission{}, &PkiError{Tag: PkiErrorVerificationFailed, VerificationFailed: *failure}
	}
	return admissionOf(cert, profile), nil
}

func (v *certificateVerifier) TslInfo() (TslInfo, *PkiError) {
	snap, err := v.current()
	if err != nil {
		return TslInfo{}, invalidInput(err.Error())
	}
	return snap.infoAt(v.nowFunc()), nil
}

func (v *certificateVerifier) SetTsl(raw []byte) (TslInfo, *PkiError) {
	snap, err := loadSnapshot(raw)
	if err != nil {
		return TslInfo{}, invalidInput(err.Error())
	}
	v.mu.Lock()
	v.snapshot.Store(snap)
	v.mu.Unlock()
	return snap.infoAt(v.nowFunc()), nil
}

func (v *certificateVerifier) RefreshTsl(ctx context.Context, url string) (*[]byte, *PkiError) {
	current := ""
	if snap, err := v.current(); err == nil {
		current = snap.hash
	}
	raw, err := fetchIfChanged(ctx, v.http, url, current)
	if err != nil {
		return nil, downloadFailed(err.Error())
	}
	if raw == nil {
		return nil, nil
	}
	if _, e := v.SetTsl(raw); e != nil {
		return nil, e
	}
	return &raw, nil
}

func (v *certificateVerifier) CacheStats() CacheStats { return v.ocsp.stats() }
