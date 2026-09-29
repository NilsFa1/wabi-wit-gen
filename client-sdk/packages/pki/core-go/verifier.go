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

func parseTrustList(raw []byte) (*trustList, error) {
	list, err := tsl.Parse(bytes.NewReader(raw), "")
	if err != nil {
		return nil, fmt.Errorf("TSL could not be parsed: %v", err)
	}
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
	tslRaw      []byte

	once    sync.Once // the TSL is parsed on first use (the constructor cannot fail)
	trust   *trustList
	tslErr  error
	http    *http.Client
	nowFunc func() time.Time
}

// newCertificateVerifier is the constructor the generated dispatcher calls.
func newCertificateVerifier(productType string, tslXML []byte) CertificateVerifier {
	return &certificateVerifier{
		productType: productType,
		tslRaw:      bytes.Clone(tslXML),
		http:        &http.Client{Timeout: 10 * time.Second},
		nowFunc:     time.Now,
	}
}

func (v *certificateVerifier) Close() {}

func (v *certificateVerifier) trustList() (*trustList, error) {
	v.once.Do(func() { v.trust, v.tslErr = parseTrustList(v.tslRaw) })
	return v.trust, v.tslErr
}

func invalidInput(m string) *PkiError { return &PkiError{Tag: PkiErrorInvalidInput, InvalidInput: m} }

func (v *certificateVerifier) Verify(ctx context.Context, der []byte, profiles []CertificateProfile, withOcsp bool) (Admission, *PkiError) {
	if len(profiles) == 0 {
		return Admission{}, invalidInput("at least one certificate profile is required")
	}
	trust, err := v.trustList()
	if err != nil {
		return Admission{}, invalidInput(err.Error())
	}
	cert, err := gempki.ParseCertificate(der)
	if err != nil {
		return Admission{}, invalidInput("certificate is not a DER-encoded X.509 certificate: " + err.Error())
	}
	c := &tucPki018{productType: v.productType, trust: trust, now: v.nowFunc().UTC()}
	if withOcsp {
		c.ocsp = &gempki.OCSPChecker{HTTPClient: v.http, TSLResponders: trust.responders, RequireCertHash: true}
	}
	profile, failure := c.verify(ctx, cert, profiles)
	if failure != nil {
		return Admission{}, &PkiError{Tag: PkiErrorVerificationFailed, VerificationFailed: *failure}
	}
	return admissionOf(cert, profile), nil
}
