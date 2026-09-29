package main

// Business logic of the gematik:zero PoC core: RFC 9449 DPoP proofs built with
// gematik libzero (JWK thumbprint) and lestrrat-go/jwx (JWS). The private key
// stays in the app: hostSigner forwards every signature to the imported WIT
// interface `host-keys` (ABI 1.1 host call), e.g. to the Secure Enclave.

import (
	"context"
	"crypto"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha256"
	"crypto/x509"
	"encoding/base64"
	"encoding/json"
	"errors"
	"io"
	"strings"
	"time"

	"github.com/gematik/zero-lab/go/libzero/util"
	"github.com/lestrrat-go/jwx/v2/jwa"
	"github.com/lestrrat-go/jwx/v2/jwk"
	"github.com/lestrrat-go/jwx/v2/jws"
)

const coreInfo = "gematik-zero-go-core/0.1.0 (libzero v0.1.0, jwx v2)"

func invalidInput(m string) *ZeroError {
	return &ZeroError{Tag: ZeroErrorInvalidInput, InvalidInput: m}
}
func cryptoError(m string) *ZeroError { return &ZeroError{Tag: ZeroErrorCrypto, Crypto: m} }

// hostError turns a failed host call into a domain error for the app.
func hostError(e *ZeroError, err error) *ZeroError {
	if e != nil {
		return e
	}
	return cryptoError("host-keys: " + err.Error())
}

type dpopSigner struct{ keyID string }

// newDpopSigner is the constructor the generated dispatcher calls.
func newDpopSigner(keyID string) DpopSigner { return &dpopSigner{keyID: keyID} }

func (s *dpopSigner) Close() {}

// publicJWK asks the app for the public key and converts it to a JWK.
func (s *dpopSigner) publicJWK(ctx context.Context) (*ecdsa.PublicKey, jwk.Key, *ZeroError) {
	der, e, err := HostKeysPublicKey(ctx, s.keyID)
	if e != nil || err != nil {
		return nil, nil, hostError(e, err)
	}
	pub, err := x509.ParsePKIXPublicKey(der)
	if err != nil {
		return nil, nil, cryptoError("public key: " + err.Error())
	}
	ec, ok := pub.(*ecdsa.PublicKey)
	if !ok || ec.Curve != elliptic.P256() {
		return nil, nil, cryptoError("public key: expected EC P-256")
	}
	key, err := jwk.FromRaw(ec)
	if err != nil {
		return nil, nil, cryptoError(err.Error())
	}
	return ec, key, nil
}

func (s *dpopSigner) Thumbprint(ctx context.Context) (string, *ZeroError) {
	_, key, e := s.publicJWK(ctx)
	if e != nil {
		return "", e
	}
	tp, err := (&util.Jwk{Key: key}).ThumbprintString(crypto.SHA256) // libzero
	if err != nil {
		return "", cryptoError(err.Error())
	}
	return tp, nil
}

func (s *dpopSigner) CreateProof(ctx context.Context, method, url string, accessToken, nonce *string) (string, *ZeroError) {
	if method == "" || !strings.HasPrefix(url, "http") {
		return "", invalidInput("http-method and an absolute http(s) http-url are required")
	}
	pub, key, e := s.publicJWK(ctx)
	if e != nil {
		return "", e
	}
	claims := map[string]any{
		"jti": randomID(),
		"htm": method,
		"htu": url,
		"iat": time.Now().Unix(),
	}
	if accessToken != nil {
		h := sha256.Sum256([]byte(*accessToken))
		claims["ath"] = base64.RawURLEncoding.EncodeToString(h[:])
	}
	if nonce != nil {
		claims["nonce"] = *nonce
	}
	payload, err := json.Marshal(claims)
	if err != nil {
		return "", cryptoError(err.Error())
	}
	hdr := jws.NewHeaders()
	_ = hdr.Set(jws.TypeKey, "dpop+jwt")
	_ = hdr.Set(jws.JWKKey, key)
	signer := &hostSigner{ctx: ctx, keyID: s.keyID, pub: pub}
	proof, err := jws.Sign(payload, jws.WithKey(jwa.ES256, signer, jws.WithProtectedHeaders(hdr)))
	if err != nil {
		var ze *ZeroError
		if errors.As(err, &ze) {
			return "", ze
		}
		return "", cryptoError(err.Error())
	}
	return string(proof), nil
}

// hostSigner is a crypto.Signer whose private key lives in the app.
type hostSigner struct {
	ctx   context.Context
	keyID string
	pub   *ecdsa.PublicKey
}

func (h *hostSigner) Public() crypto.PublicKey { return h.pub }

func (h *hostSigner) Sign(_ io.Reader, digest []byte, _ crypto.SignerOpts) ([]byte, error) {
	sig, e, err := HostKeysSign(h.ctx, h.keyID, digest)
	if e != nil {
		return nil, e
	}
	return sig, err // ASN.1 DER; jwx converts it to JOSE r||s
}

func randomID() string {
	b := make([]byte, 16)
	_, _ = rand.Read(b)
	return base64.RawURLEncoding.EncodeToString(b)
}
