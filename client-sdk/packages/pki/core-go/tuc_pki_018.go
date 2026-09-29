// SPDX-License-Identifier: EUPL-1.2

package main

// TUC_PKI_018 ("Zertifikatsprüfung in der TI") in the classic TSL trust
// model, transcribed from gemLibPki 5.0.2 (TucPki018Verifier and its
// validators) — same order of checks, same error codes and texts:
//
//	issuer in the TSL           TspInformationProvider             TE_1027 / SE_1023
//	common checks               CertificateCommonVerification
//	  validity                    ValidityValidator                SE_1021
//	  signature                   SignatureValidator               SE_1024
//	  issuer status in TSL        IssuerServiceStatusValidator     SE_1036
//	OCSP (if requested)         TucPki018OcspValidator             TE_1026, TE_1032, SE_1030, SE_1031, SW_1047, TW_1044
//	per profile, first match    CertificateProfileVerification
//	  key usage                   KeyUsageValidator                SE_1016
//	  extended key usage          ExtendedKeyUsageValidator        SE_1017
//	  certificate type            ...ByCertificateTypeOidValidator SE_1033 / SE_1018
//	  CA authorized for type      ...InIssuerTspServiceExtension   SE_1061
//	  critical extensions         CriticalExtensionsValidator      CUSTOM_CERTIFICATE_EXCEPTION
//
// Only the ECC profiles are implemented; RSA profiles are reported as not
// supported by this core.

import (
	"bytes"
	"context"
	"crypto/x509"
	"encoding/asn1"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/gematik/zero-lab/go/gempki"
)

// gemLibPki error codes (ErrorCode.java) with their classifier and text.
type pkiCode struct{ name, classifier, text string }

var (
	te1026 = pkiCode{"TE_1026_SERVICESUPPLYPOINT_MISSING", "TECHNICAL_ERROR", "Das Element „Service-Supply Point“ konnte nicht gefunden werden"}
	te1027 = pkiCode{"TE_1027_CA_CERT_MISSING", "TECHNICAL_ERROR", "CA kann nicht in den TSL-Informationen ermittelt werden"}
	se1023 = pkiCode{"SE_1023_AUTHORITYKEYID_DIFFERENT", "SECURITY_ERROR", "Authority-Key-Identifier des End-Entity-Zertifikats von Subject-Key-Identifier des CA-Zertifikats unterschiedlich"}
	se1021 = pkiCode{"SE_1021_CERTIFICATE_NOT_VALID_TIME", "SECURITY_ERROR", "Zertifikat ist zeitlich nicht gültig"}
	se1024 = pkiCode{"SE_1024_CERTIFICATE_NOT_VALID_MATH", "SECURITY_ERROR", "Zertifikats-Signatur ist mathematisch nicht gültig"}
	se1036 = pkiCode{"SE_1036_CA_CERTIFICATE_REVOKED_IN_TSL", "SECURITY_ERROR", "Das Zertifikat ist ungültig. Es wurde nach der Sperrung der ausgebenden CA ausgestellt"}
	te1029 = pkiCode{"TE_1029_OCSP_CHECK_REVOCATION_ERROR", "TECHNICAL_ERROR", "Die OCSP-Prüfung konnte nicht durchgeführt werden (2)"}
	se1030 = pkiCode{"SE_1030_OCSP_CERT_MISSING", "SECURITY_ERROR", "OCSP-Zertifikat nicht in TSL-Informationen enthalten"}
	se1031 = pkiCode{"SE_1031_OCSP_SIGNATURE_ERROR", "SECURITY_ERROR", "Signatur der Response ist nicht gültig"}
	te1032 = pkiCode{"TE_1032_OCSP_NOT_AVAILABLE", "TECHNICAL_ERROR", "OCSP-Responder nicht verfügbar"}
	tw1044 = pkiCode{"TW_1044_CERT_UNKNOWN", "TECHNICAL_WARNING", "Warnung, dass zum angefragten Zertifikat keine Statusinformationen verfügbar sind"}
	sw1047 = pkiCode{"SW_1047_CERT_REVOKED", "SECURITY_WARNING", "Das Zertifikat wurde vor oder zum Referenzzeitpunkt widerrufen"}
	se1016 = pkiCode{"SE_1016_WRONG_KEYUSAGE", "SECURITY_ERROR", "KeyUsage ist nicht vorhanden bzw. entspricht nicht der vorgesehenen KeyUsage"}
	se1017 = pkiCode{"SE_1017_WRONG_EXTENDEDKEYUSAGE", "SECURITY_ERROR", "Extended-KeyUsage entspricht nicht der vorgesehenen Extended-KeyUsage"}
	se1018 = pkiCode{"SE_1018_CERT_TYPE_MISMATCH", "SECURITY_ERROR", "Zertifikatstyp-OID stimmt nicht überein"}
	se1033 = pkiCode{"SE_1033_CERT_TYPE_INFO_MISSING", "SECURITY_ERROR", "Kein Element PolicyIdentifier vorhanden"}
	se1061 = pkiCode{"SE_1061_CERT_TYPE_CA_NOT_AUTHORIZED", "SECURITY_ERROR", "CA (laut TSL) nicht autorisiert für die Herausgabe dieses Zertifikatstyps"}
	custom = pkiCode{"CUSTOM_CERTIFICATE_EXCEPTION", "SECURITY_ERROR", "Custom certificate exception"}
	// Not a gemLibPki code: this core implements the ECC profiles only.
	rsaUnsupported = pkiCode{"UNSUPPORTED_RSA_PROFILE", "TECHNICAL_ERROR", "RSA-Zertifikatsprofile werden von diesem Core nicht unterstützt (nur ECC)"}
)

// pkiFailure is one failed check; the message is gemLibPki's format.
type pkiFailure struct {
	code   pkiCode
	detail string // optional extra context, appended to the message
}

func (f *pkiFailure) message(productType string) string {
	m := fmt.Sprintf("%s:PKI - %s %s - %s", productType, f.code.classifier, f.code.name, f.code.text)
	if f.detail != "" {
		m += " (" + f.detail + ")"
	}
	return m
}

func fail(code pkiCode) *pkiFailure { return &pkiFailure{code: code} }

// Key usage bits in the order of the X.509 KeyUsage BIT STRING.
const (
	kuDigitalSignature = x509.KeyUsageDigitalSignature
	kuNonRepudiation   = x509.KeyUsageContentCommitment
	kuKeyAgreement     = x509.KeyUsageKeyAgreement
)

// EKU OIDs used by the ECC profiles.
const (
	ekuClientAuth      = "1.3.6.1.5.5.7.3.2"
	ekuEmailProtection = "1.3.6.1.5.5.7.3.4"
	ekuTslSigning      = "0.4.0.2231.3.0"
)

// profileSpec is gemLibPki's CertificateProfile: certificate type OID
// (empty = CERT_TYPE_ANY), intended key usages and extended key usages.
type profileSpec struct {
	name             string
	typeOID          string
	keyUsages        []x509.KeyUsage
	extKeyUsages     []string
	failOnMissingEku bool
	skipTypeChecks   bool // CERT_PROFILE_C_TSL_SIG
}

var eccProfiles = map[CertificateProfile]profileSpec{
	CertificateProfileCChAutEcc:  {"CERT_PROFILE_C_CH_AUT_ECC", "1.2.276.0.76.4.70", []x509.KeyUsage{kuDigitalSignature}, []string{ekuClientAuth}, false, false},
	CertificateProfileCHpAutEcc:  {"CERT_PROFILE_C_HP_AUT_ECC", "1.2.276.0.76.4.75", []x509.KeyUsage{kuDigitalSignature, kuKeyAgreement}, []string{ekuClientAuth, ekuEmailProtection}, true, false},
	CertificateProfileCHciAutEcc: {"CERT_PROFILE_C_HCI_AUT_ECC", "1.2.276.0.76.4.77", []x509.KeyUsage{kuDigitalSignature}, []string{ekuClientAuth}, true, false},
	CertificateProfileCHciEncEcc: {"CERT_PROFILE_C_HCI_ENC_ECC", "1.2.276.0.76.4.76", []x509.KeyUsage{kuKeyAgreement}, nil, false, false},
	CertificateProfileCHciOsig:   {"CERT_PROFILE_C_HCI_OSIG", "1.2.276.0.76.4.78", []x509.KeyUsage{kuNonRepudiation}, nil, false, false},
	CertificateProfileCFdSig:     {"CERT_PROFILE_C_FD_SIG", "1.2.276.0.76.4.203", []x509.KeyUsage{kuDigitalSignature}, nil, false, false},
	CertificateProfileCFdOsig:    {"CERT_PROFILE_C_FD_OSIG", "1.2.276.0.76.4.283", []x509.KeyUsage{kuNonRepudiation}, nil, false, false},
	CertificateProfileCTslSig:    {"CERT_PROFILE_C_TSL_SIG", "", []x509.KeyUsage{kuNonRepudiation}, []string{ekuTslSigning}, true, true},
}

var (
	oidKeyUsage            = asn1.ObjectIdentifier{2, 5, 29, 15}
	oidBasicConstraints    = asn1.ObjectIdentifier{2, 5, 29, 19}
	oidCertificatePolicies = asn1.ObjectIdentifier{2, 5, 29, 32}
	oidExtendedKeyUsage    = asn1.ObjectIdentifier{2, 5, 29, 37}
)

type tucPki018 struct {
	productType string
	trust       *trustList
	now         time.Time
	ocsp        *gempki.OCSPChecker // nil: no OCSP check (SW_1039)
}

// verify runs TUC_PKI_018 and returns the profile that matched, or the
// failure: the first error, with the messages of all profiles.
func (c *tucPki018) verify(ctx context.Context, cert *x509.Certificate, profiles []CertificateProfile) (CertificateProfile, *VerificationFailure) {
	failed := func(f *pkiFailure) *VerificationFailure {
		return &VerificationFailure{Code: f.code.name, Message: f.message(c.productType)}
	}
	issuer, f := c.issuerService(cert)
	if f != nil {
		return 0, failed(f)
	}
	if f := c.commonChecks(cert, issuer); f != nil {
		return 0, failed(f)
	}
	if c.ocsp != nil {
		if f := c.ocspCheck(ctx, cert, issuer); f != nil {
			return 0, failed(f)
		}
	}
	var first *pkiFailure
	var messages []string
	for _, p := range profiles {
		f := c.profileChecks(cert, issuer, p)
		if f == nil {
			return p, nil
		}
		if first == nil {
			first = f
		}
		messages = append(messages, fmt.Sprintf("%s (%s)", f.message(c.productType), profileName(p)))
	}
	return 0, &VerificationFailure{Code: first.code.name, Message: strings.Join(messages, "; ")}
}

func profileName(p CertificateProfile) string {
	if s, ok := eccProfiles[p]; ok {
		return s.name
	}
	return fmt.Sprintf("profile %d (RSA)", uint32(p))
}

// issuerService finds the issuing CA in the TSL: same DN, and the EE's
// authority key identifier equal to the CA's subject key identifier.
func (c *tucPki018) issuerService(cert *x509.Certificate) (*caService, *pkiFailure) {
	var sameName bool
	for _, svc := range c.trust.cas {
		if !bytes.Equal(cert.RawIssuer, svc.cert.RawSubject) {
			continue
		}
		sameName = true
		if len(cert.AuthorityKeyId) > 0 && bytes.Equal(cert.AuthorityKeyId, svc.cert.SubjectKeyId) {
			return svc, nil
		}
	}
	if sameName {
		return nil, fail(se1023)
	}
	return nil, fail(te1027)
}

func (c *tucPki018) commonChecks(cert *x509.Certificate, issuer *caService) *pkiFailure {
	if c.now.Before(cert.NotBefore) || c.now.After(cert.NotAfter) {
		return fail(se1021)
	}
	// ParseCertificate (gempki/brainpool) makes this work for Brainpool keys too.
	if err := cert.CheckSignatureFrom(issuer.cert); err != nil {
		return &pkiFailure{code: se1024, detail: err.Error()}
	}
	if issuer.status == svcStatusRevoked && issuer.statusStart.Before(cert.NotBefore) {
		return fail(se1036)
	}
	return nil
}

// ocspCheck asks the responder the TSL names for the issuer (its
// ServiceSupplyPoint), accepting the responder certificates the TSL lists.
func (c *tucPki018) ocspCheck(ctx context.Context, cert *x509.Certificate, issuer *caService) *pkiFailure {
	if len(issuer.supplyPoints) == 0 {
		return fail(te1026)
	}
	checker := *c.ocsp
	checker.ResponderURL = strings.TrimSpace(issuer.supplyPoints[0])
	checker.Clock = func() time.Time { return c.now }
	res, err := checker.Check(ctx, cert, issuer.cert)
	if err != nil {
		var ve *gempki.ValidationError
		if errors.As(err, &ve) {
			switch ve.Code {
			case gempki.ErrCodeOCSPUnavailable:
				return &pkiFailure{code: te1032, detail: ve.Error()}
			case gempki.ErrCodeOCSPResponderUntrusted:
				return &pkiFailure{code: se1030, detail: ve.Error()}
			case gempki.ErrCodeOCSPResponseInvalid:
				return &pkiFailure{code: se1031, detail: ve.Error()}
			}
		}
		return &pkiFailure{code: te1029, detail: err.Error()}
	}
	switch res.Status {
	case gempki.RevocationStatusGood:
		return nil
	case gempki.RevocationStatusRevoked:
		return &pkiFailure{code: sw1047, detail: res.Reason}
	default:
		return &pkiFailure{code: tw1044, detail: res.Reason}
	}
}

func (c *tucPki018) profileChecks(cert *x509.Certificate, issuer *caService, p CertificateProfile) *pkiFailure {
	spec, ok := eccProfiles[p]
	if !ok {
		return fail(rsaUnsupported)
	}
	// KeyUsageValidator: every intended bit set (further bits are not checked).
	if !hasExtension(cert, oidKeyUsage) {
		return fail(se1016)
	}
	for _, ku := range spec.keyUsages {
		if cert.KeyUsage&ku == 0 {
			return fail(se1016)
		}
	}
	// ExtendedKeyUsageValidator: overlap with the intended EKUs and the same count.
	if len(spec.extKeyUsages) > 0 && spec.failOnMissingEku {
		eeEkus := extKeyUsageOIDs(cert)
		if eeEkus == nil {
			return fail(se1017)
		}
		overlap := 0
		for _, e := range eeEkus {
			for _, want := range spec.extKeyUsages {
				if e == want {
					overlap++
					break
				}
			}
		}
		if overlap == 0 || len(eeEkus) != len(spec.extKeyUsages) {
			return fail(se1017)
		}
	}
	if !spec.skipTypeChecks {
		// CertificateProfileByCertificateTypeOidValidator
		policies, f := policyOIDs(cert)
		if f != nil {
			return f
		}
		if spec.typeOID != "" && !policies[spec.typeOID] {
			return fail(se1018)
		}
		// CertificateTypeOidInIssuerTspServiceExtensionValidator
		authorized := false
		for oid := range policies {
			if issuer.typeOIDs[oid] {
				authorized = true
				break
			}
		}
		if !authorized {
			return fail(se1061)
		}
	}
	// CriticalExtensionsValidator: exactly basicConstraints and keyUsage (gemSpec_PKI ch. 5).
	critical := map[string]bool{}
	for _, ext := range cert.Extensions {
		if ext.Critical {
			critical[ext.Id.String()] = true
		}
	}
	if len(critical) != 2 || !critical[oidBasicConstraints.String()] || !critical[oidKeyUsage.String()] {
		return fail(custom)
	}
	return nil
}

func hasExtension(cert *x509.Certificate, oid asn1.ObjectIdentifier) bool {
	for _, ext := range cert.Extensions {
		if ext.Id.Equal(oid) {
			return true
		}
	}
	return false
}

// extKeyUsageOIDs returns the EKU OIDs of the certificate, nil without the extension.
func extKeyUsageOIDs(cert *x509.Certificate) []string {
	for _, ext := range cert.Extensions {
		if !ext.Id.Equal(oidExtendedKeyUsage) {
			continue
		}
		var oids []asn1.ObjectIdentifier
		if rest, err := asn1.Unmarshal(ext.Value, &oids); err != nil || len(rest) > 0 {
			return []string{}
		}
		out := make([]string, len(oids))
		for i, o := range oids {
			out[i] = o.String()
		}
		return out
	}
	return nil
}

// policyOIDs is gemLibPki's getCertificatePolicyOids: SE_1033 without a policy.
func policyOIDs(cert *x509.Certificate) (map[string]bool, *pkiFailure) {
	if !hasExtension(cert, oidCertificatePolicies) || len(cert.PolicyIdentifiers) == 0 {
		return nil, fail(se1033)
	}
	out := map[string]bool{}
	for _, oid := range cert.PolicyIdentifiers {
		out[oid.String()] = true
	}
	return out, nil
}
