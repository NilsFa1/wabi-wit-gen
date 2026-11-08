package main

// One session with the backend, over plain HTTP/1.1. The protocol is the one
// of the Rust core (core-rust/src/protocol): every body is sealed with
// AES-256-GCM under a key derived from the shared secret, every request is
// signed by the app-held key (host call), and one long-lived response delivers
// the backend's events as server-sent events.

import (
	"bufio"
	"bytes"
	"context"
	"crypto/aes"
	"crypto/cipher"
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

const (
	pathSessions  = "/v1/sessions"
	pathEvents    = "/v1/events"
	pathWhoami    = "/v1/whoami"
	pathCustomers = "/v1/customers"
	pathCommands  = "/v1/commands"

	headerSession   = "x-demo-session"
	headerKey       = "x-demo-key"
	headerSignature = "x-demo-signature"

	aadRequest  = "req"
	aadResponse = "res"
	aadEvent    = "evt"
)

type whoamiDto struct {
	Key string `json:"key"`
}

type commandDto struct {
	Name    string `json:"name"`
	Payload []byte `json:"payload"`
}

type errorBody struct {
	Code    string  `json:"code"`
	Message *string `json:"message"`
}

type eventBody struct {
	Kind     string   `json:"kind"`
	Customer Customer `json:"customer"`
	Text     string   `json:"text"`
}

// All sessions share one connection pool.
var httpClient = &http.Client{Transport: &http.Transport{MaxIdleConnsPerHost: 256}}

// bodyKey derives the AES-256-GCM key: HMAC-SHA256(api-key, "demo-client/v1 body key").
func bodyKey(apiKey string) cipher.AEAD {
	mac := hmac.New(sha256.New, []byte(apiKey))
	mac.Write([]byte("demo-client/v1 body key"))
	block, _ := aes.NewCipher(mac.Sum(nil))
	aead, _ := cipher.NewGCM(block)
	return aead
}

// seal returns nonce || ciphertext || tag.
func seal(aead cipher.AEAD, aad string, plaintext []byte) []byte {
	nonce := make([]byte, aead.NonceSize())
	_, _ = rand.Read(nonce)
	return aead.Seal(nonce, nonce, plaintext, []byte(aad))
}

func open(aead cipher.AEAD, aad string, sealed []byte) ([]byte, bool) {
	n := aead.NonceSize()
	if len(sealed) < n {
		return nil, false
	}
	plaintext, err := aead.Open(nil, sealed[:n], sealed[n:], []byte(aad))
	return plaintext, err == nil
}

// requestDigest is what the app-held key signs.
func requestDigest(method, path, session string, body []byte) []byte {
	h := sha256.New()
	for _, part := range []string{method, path, session} {
		h.Write([]byte(part))
		h.Write([]byte("\n"))
	}
	h.Write(body)
	return h.Sum(nil)
}

// queryValue percent-encodes everything but RFC 3986 unreserved characters.
func queryValue(s string) string {
	var b strings.Builder
	for i := 0; i < len(s); i++ {
		c := s[i]
		if c >= 'A' && c <= 'Z' || c >= 'a' && c <= 'z' || c >= '0' && c <= '9' || strings.IndexByte("-._~", c) >= 0 {
			b.WriteByte(c)
		} else {
			fmt.Fprintf(&b, "%%%02X", c)
		}
	}
	return b.String()
}

type session struct {
	link  *transport
	aead  cipher.AEAD
	keyID *string // app-held key that signs every request
	id    string
	// ctx ends with the event channel: by end() or by the backend.
	ctx context.Context
	end context.CancelFunc
}

func (s *session) alive() bool { return s.ctx.Err() == nil }

func openSession(ctx context.Context, o ClientOptions, link *transport, publish func(ClientEvent)) (*session, *ClientError) {
	if !strings.HasPrefix(link.base, "http://") && !strings.HasPrefix(link.base, "https://") {
		return nil, networkError("unsupported endpoint (the Go core speaks http:// only): " + link.base)
	}
	apiKey := ""
	if o.ApiKey != nil {
		apiKey = *o.ApiKey
	}
	s := &session{link: link, aead: bodyKey(apiKey), keyID: o.KeyId}
	s.ctx, s.end = context.WithCancel(context.Background())
	timeout := time.Duration(o.RequestTimeoutMs) * time.Millisecond

	headers := map[string]string{}
	if s.keyID != nil {
		// The app's keystore answers; the private key never comes here.
		spki, e, err := HostKeysPublicKey(ctx, *s.keyID)
		if e != nil || err != nil {
			s.end()
			return nil, hostError("public-key", e, err)
		}
		headers[headerKey] = base64.StdEncoding.EncodeToString(spki)
	}
	var opened struct {
		Session string `json:"session"`
	}
	if e := s.exchange(ctx, "POST", pathSessions, headers, map[string]string{"client": "demo-go-core"}, &opened, timeout); e != nil {
		s.end()
		return nil, e
	}
	s.id = opened.Session

	// The event channel lives as long as the session; only its start is awaited.
	start, cancelStart := context.WithCancel(s.ctx)
	stop := context.AfterFunc(ctx, cancelStart)
	resp, e := s.send(start, ctx, "GET", pathEvents, nil, nil)
	if !stop() && e == nil { // the host cancelled connect() meanwhile
		resp.Body.Close()
		e = errDisconnected
	}
	if e == nil && resp.StatusCode != 200 {
		resp.Body.Close()
		e = protocolError(fmt.Sprintf("event channel: HTTP %d", resp.StatusCode))
	}
	if e != nil {
		s.end()
		return nil, e
	}
	go s.pump(resp.Body, publish)
	return s, nil
}

// hostError turns a failed host call into a domain error for the app.
func hostError(what string, e *ClientError, err error) *ClientError {
	if e != nil {
		return e
	}
	return keyError("host-keys." + what + ": " + err.Error())
}

// send seals the body, signs the request and returns the started response.
// `ctx` bounds the HTTP exchange, `host` the host call for the signature.
func (s *session) send(ctx, host context.Context, method, path string, headers map[string]string, body any) (*http.Response, *ClientError) {
	var sealed []byte
	if body != nil {
		plain, err := json.Marshal(body)
		if err != nil {
			return nil, protocolError(err.Error())
		}
		sealed = seal(s.aead, aadRequest, plain)
	}
	s.link.requests.Add(1)
	req, err := http.NewRequestWithContext(ctx, method, strings.TrimRight(s.link.base, "/")+path, bytes.NewReader(sealed))
	if err != nil {
		return nil, networkError(err.Error())
	}
	for name, value := range headers {
		req.Header.Set(name, value)
	}
	if s.id != "" {
		req.Header.Set(headerSession, s.id)
	}
	if s.keyID != nil {
		signature, e, err := HostKeysSign(host, *s.keyID, requestDigest(method, path, s.id, sealed))
		if e != nil || err != nil {
			return nil, hostError("sign", e, err)
		}
		req.Header.Set(headerSignature, base64.StdEncoding.EncodeToString(signature))
	}
	resp, err := httpClient.Do(req)
	if err != nil {
		return nil, networkError(err.Error())
	}
	return resp, nil
}

// exchange runs one request/response and decodes the sealed JSON answer into out.
func (s *session) exchange(ctx context.Context, method, path string, headers map[string]string, body, out any, timeout time.Duration) *ClientError {
	// The request ends with the host's ctx (cancellation), the session, or the timeout.
	reqCtx, cancel := context.WithCancel(ctx)
	defer cancel()
	defer context.AfterFunc(s.ctx, cancel)()
	if timeout > 0 {
		var cancelTimeout context.CancelFunc
		reqCtx, cancelTimeout = context.WithTimeout(reqCtx, timeout)
		defer cancelTimeout()
	}
	failed := func(e *ClientError) *ClientError {
		switch {
		case !s.alive() || ctx.Err() != nil: // (if the host cancelled, the result is discarded anyway)
			return errDisconnected
		case errors.Is(reqCtx.Err(), context.DeadlineExceeded):
			return networkError(fmt.Sprintf("request timed out after %d ms", timeout.Milliseconds()))
		}
		return e
	}
	resp, e := s.send(reqCtx, reqCtx, method, path, headers, body)
	if e != nil {
		return failed(e)
	}
	defer resp.Body.Close()
	sealed, err := io.ReadAll(resp.Body)
	if err != nil {
		return failed(networkError(err.Error()))
	}
	plain, ok := open(s.aead, aadResponse, sealed)
	switch {
	case resp.StatusCode == 200 && ok:
		if err := json.Unmarshal(plain, out); err != nil {
			return protocolError("unexpected response shape: " + err.Error())
		}
		return nil
	case resp.StatusCode == 200:
		return protocolError("response cannot be decrypted")
	case resp.StatusCode == 401: // a backend with another key cannot even tell us why
		return errUnauthorized
	case !ok:
		return protocolError(fmt.Sprintf("HTTP %d", resp.StatusCode))
	}
	var eb errorBody
	if err := json.Unmarshal(plain, &eb); err != nil {
		return protocolError("unexpected response shape: " + err.Error())
	}
	switch eb.Code {
	case "not-found":
		return errNotFound
	case "unauthorized":
		return errUnauthorized
	}
	if eb.Message != nil {
		return protocolError(*eb.Message)
	}
	return protocolError(eb.Code)
}

func (s *session) call(ctx context.Context, method, path string, body, out any, timeout time.Duration) *ClientError {
	return s.exchange(ctx, method, path, nil, body, out, timeout)
}

// pump reads server-sent events (`data: <base64 sealed JSON>`) until the
// response ends or the session is ended.
func (s *session) pump(body io.ReadCloser, publish func(ClientEvent)) {
	defer s.end()
	defer body.Close()
	lines := bufio.NewReader(body)
	for {
		line, err := lines.ReadString('\n')
		if err != nil {
			return
		}
		data, ok := strings.CutPrefix(strings.TrimRight(line, "\r\n"), "data: ")
		if !ok {
			continue // comments, keep-alives, blank lines
		}
		sealed, err := base64.StdEncoding.DecodeString(data)
		if err != nil {
			continue
		}
		plain, ok := open(s.aead, aadEvent, sealed)
		var ev eventBody
		if !ok || json.Unmarshal(plain, &ev) != nil {
			continue // not for us or from a newer backend: skip
		}
		switch ev.Kind {
		case "customer-updated":
			publish(ClientEvent{Tag: ClientEventCustomerUpdated, CustomerUpdated: ev.Customer})
		case "message-received":
			publish(ClientEvent{Tag: ClientEventMessageReceived, MessageReceived: ev.Text})
		}
	}
}
