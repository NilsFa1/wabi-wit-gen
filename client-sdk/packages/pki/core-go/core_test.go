// SPDX-License-Identifier: EUPL-1.2

package main

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
)

func fixture(t *testing.T, name string) []byte {
	t.Helper()
	b, err := os.ReadFile(filepath.Join("..", "tests", "fixtures", name))
	if err != nil {
		t.Fatal(err)
	}
	return b
}

var aut = []CertificateProfile{CertificateProfileCHciAutEcc}

func verifier(t *testing.T) CertificateVerifier {
	return newCertificateVerifier("PoC-Test", fixture(t, "tsl.xml"))
}

func failureCode(t *testing.T, cert []byte, profiles []CertificateProfile) string {
	t.Helper()
	_, e := verifier(t).Verify(context.Background(), cert, profiles, false)
	if e == nil {
		t.Fatal("expected a failure")
	}
	if e.Tag != PkiErrorVerificationFailed {
		t.Fatalf("expected verification-failed, got %+v", *e)
	}
	return e.VerificationFailed.Code
}

func TestValidSmcB(t *testing.T) {
	a, e := verifier(t).Verify(context.Background(), fixture(t, "valid.der"), aut, false)
	if e != nil {
		t.Fatalf("%+v", *e)
	}
	if a.Profile != CertificateProfileCHciAutEcc || a.RegistrationNumber != "1-2-ARZT-Test-01" ||
		strings.Join(a.ProfessionItems, ",") != "Betriebsstätte Arzt" ||
		strings.Join(a.ProfessionOids, ",") != "1.2.276.0.76.4.50" ||
		a.AdmissionAuthority != "CN=Kassenärztliche Vereinigung Test,C=DE" {
		t.Fatalf("%+v", a)
	}
}

func TestFirstMatchingProfileWins(t *testing.T) {
	a, e := verifier(t).Verify(context.Background(), fixture(t, "valid.der"),
		[]CertificateProfile{CertificateProfileCHpAutEcc, CertificateProfileCHciAutEcc}, false)
	if e != nil || a.Profile != CertificateProfileCHciAutEcc {
		t.Fatalf("%+v %+v", a, e)
	}
}

func TestFailureCodes(t *testing.T) {
	for _, tc := range []struct{ fixture, code string }{
		{"expired.der", "SE_1021_CERTIFICATE_NOT_VALID_TIME"},
		{"wrong-key-usage.der", "SE_1016_WRONG_KEYUSAGE"},
		{"unknown-ca.der", "TE_1027_CA_CERT_MISSING"},
	} {
		if got := failureCode(t, fixture(t, tc.fixture), aut); got != tc.code {
			t.Errorf("%s: got %s, want %s", tc.fixture, got, tc.code)
		}
	}
}

func TestWrongProfileIsTypeMismatch(t *testing.T) {
	// a C.HCI.AUT certificate checked as C.FD.SIG: key usage fits, type OID does not
	if got := failureCode(t, fixture(t, "valid.der"), []CertificateProfile{CertificateProfileCFdSig}); got != "SE_1018_CERT_TYPE_MISMATCH" {
		t.Fatal(got)
	}
}

func TestRsaProfileNotSupported(t *testing.T) {
	if got := failureCode(t, fixture(t, "valid.der"), []CertificateProfile{CertificateProfileCHciAutRsa}); got != "UNSUPPORTED_RSA_PROFILE" {
		t.Fatal(got)
	}
}

func TestMessageFormat(t *testing.T) {
	_, e := verifier(t).Verify(context.Background(), fixture(t, "expired.der"), aut, false)
	// a common check (before the profile loop): gemLibPki's message, no profile suffix
	want := "PoC-Test:PKI - SECURITY_ERROR SE_1021_CERTIFICATE_NOT_VALID_TIME - Zertifikat ist zeitlich nicht gültig"
	if e == nil || e.VerificationFailed.Message != want {
		t.Fatalf("%+v", e)
	}
}

func TestInvalidInput(t *testing.T) {
	for name, call := range map[string]func() *PkiError{
		"certificate": func() *PkiError {
			_, e := verifier(t).Verify(context.Background(), []byte{1, 2, 3}, aut, false)
			return e
		},
		"no profiles": func() *PkiError {
			_, e := verifier(t).Verify(context.Background(), fixture(t, "valid.der"), nil, false)
			return e
		},
		"tsl": func() *PkiError {
			_, e := newCertificateVerifier("x", []byte("<no-tsl/>")).Verify(context.Background(), fixture(t, "valid.der"), aut, false)
			return e
		},
	} {
		if e := call(); e == nil || e.Tag != PkiErrorInvalidInput {
			t.Errorf("%s: %+v", name, e)
		}
	}
}

func TestConcurrentVerifications(t *testing.T) {
	v := verifier(t)
	cert := fixture(t, "valid.der")
	var wg sync.WaitGroup
	errs := make(chan string, 64)
	for i := 0; i < 64; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if a, e := v.Verify(context.Background(), cert, aut, false); e != nil || a.RegistrationNumber != "1-2-ARZT-Test-01" {
				errs <- "verification failed"
			}
		}()
	}
	wg.Wait()
	close(errs)
	for e := range errs {
		t.Fatal(e)
	}
}
