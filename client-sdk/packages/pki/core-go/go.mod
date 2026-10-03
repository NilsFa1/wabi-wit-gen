module gematik.example/pkicore

go 1.26.4

require (
	wabi.example/wabiabi v0.0.0
	github.com/gematik/zero-lab/go/gempki v0.23.0
	golang.org/x/crypto v0.57.0
)

require github.com/gematik/zero-lab/go/brainpool v1.1.0 // indirect

replace wabi.example/wabiabi => ../../../runtime/go
