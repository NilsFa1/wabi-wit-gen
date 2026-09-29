// SPDX-License-Identifier: EUPL-1.2

package main

// The admission (Rolle) of a verified certificate. Profession items, OIDs and
// registration number come from gempki; the admission authority, which
// gempki does not expose, is read here from the AdmissionSyntax
// (Common PKI / ISIS-MTT, id-isismtt-at-admission) and formatted like
// gemLibPki (BouncyCastle X500Name: RDNs in certificate order).

import (
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/asn1"
	"fmt"
	"strings"

	"github.com/gematik/zero-lab/go/gempki"
)

var oidAdmission = asn1.ObjectIdentifier{1, 3, 36, 8, 3, 3}

func admissionOf(cert *x509.Certificate, profile CertificateProfile) Admission {
	a := Admission{Profile: profile, ProfessionItems: []string{}, ProfessionOids: []string{}}
	if st, err := gempki.ParseAdmissionStatement(cert); err == nil && st != nil {
		a.ProfessionItems = append(a.ProfessionItems, st.ProfessionItems...)
		a.ProfessionOids = append(a.ProfessionOids, st.ProfessionOids...)
		a.RegistrationNumber = st.RegistrationNumber
	}
	a.AdmissionAuthority = admissionAuthority(cert)
	return a
}

// admissionAuthority returns the directoryName of AdmissionSyntax.admissionAuthority,
// or "" if the certificate names none.
func admissionAuthority(cert *x509.Certificate) string {
	for _, ext := range cert.Extensions {
		if !ext.Id.Equal(oidAdmission) {
			continue
		}
		var syntax asn1.RawValue // AdmissionSyntax ::= SEQUENCE { admissionAuthority GeneralName OPTIONAL, ... }
		if _, err := asn1.Unmarshal(ext.Value, &syntax); err != nil {
			return ""
		}
		var first asn1.RawValue
		if _, err := asn1.Unmarshal(syntax.Bytes, &first); err != nil {
			return ""
		}
		// GeneralName.directoryName is [4] EXPLICIT Name
		if first.Class != asn1.ClassContextSpecific || first.Tag != 4 {
			return ""
		}
		var rdns pkix.RDNSequence
		if _, err := asn1.Unmarshal(first.Bytes, &rdns); err != nil {
			return ""
		}
		return formatName(rdns)
	}
	return ""
}

var attrNames = map[string]string{
	"2.5.4.3": "CN", "2.5.4.6": "C", "2.5.4.7": "L", "2.5.4.8": "ST", "2.5.4.10": "O",
	"2.5.4.11": "OU", "2.5.4.5": "SERIALNUMBER", "2.5.4.4": "SURNAME", "2.5.4.42": "GIVENNAME",
}

// formatName prints the RDNs in certificate order, like BouncyCastle's X500Name.toString().
func formatName(rdns pkix.RDNSequence) string {
	var parts []string
	for _, rdn := range rdns {
		var attrs []string
		for _, atv := range rdn {
			key, ok := attrNames[atv.Type.String()]
			if !ok {
				key = atv.Type.String()
			}
			attrs = append(attrs, key+"="+escapeRDN(fmt.Sprint(atv.Value)))
		}
		parts = append(parts, strings.Join(attrs, "+"))
	}
	return strings.Join(parts, ",")
}

func escapeRDN(s string) string {
	r := strings.NewReplacer(`\`, `\\`, `,`, `\,`, `+`, `\+`, `"`, `\"`, `<`, `\<`, `>`, `\>`, `;`, `\;`, `=`, `\=`)
	return r.Replace(s)
}
