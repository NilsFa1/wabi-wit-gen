package main

import (
	"encoding/hex"
	"testing"
)

// The same known answers as core-rust/src/crypto: both cores must agree.
func TestKnownAnswers(t *testing.T) {
	aead := bodyKey("demo-key")
	nonce := make([]byte, 12)
	sealed := aead.Seal(nonce, nonce, []byte("hello"), []byte(aadRequest))
	if got := hex.EncodeToString(sealed); got != "000000000000000000000000e01f22429807d0ba0d6b42659676e605a1545ae5f4" {
		t.Errorf("sealed body: %s", got)
	}
	if plain, ok := open(aead, aadRequest, sealed); !ok || string(plain) != "hello" {
		t.Errorf("open: %q %v", plain, ok)
	}
	if _, ok := open(aead, aadResponse, sealed); ok {
		t.Error("a body must be bound to its role")
	}
	digest := requestDigest("POST", "/v1/customers", "abc", []byte("body"))
	if got := hex.EncodeToString(digest); got != "4807155a56ea86d92eae1a05098bd891b459ea1d82e4c0c9c915b7d3b47237d7" {
		t.Errorf("request digest: %s", got)
	}
	if got := queryValue("nö/../x~1"); got != "n%C3%B6%2F..%2Fx~1" {
		t.Errorf("query value: %s", got)
	}
}
