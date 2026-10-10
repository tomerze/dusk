package reconcile

import (
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"strconv"
)

var (
	ErrUnknownKey         = errors.New("no configured ledger key has this key id")
	ErrMalformedSignature = errors.New("the signature is not base64 of 64 bytes")
	ErrInvalidSignature   = errors.New("the signature does not verify")
)

type VerifyingKeys struct {
	keys map[string]ed25519.PublicKey
}

func Thumbprint(public ed25519.PublicKey) string {
	members := `{"crv":"Ed25519","kty":"OKP","x":"` + base64.RawURLEncoding.EncodeToString(public) + `"}`
	digest := sha256.Sum256([]byte(members))
	return base64.RawURLEncoding.EncodeToString(digest[:])
}

func ParseVerifyingKeys(document []byte) (*VerifyingKeys, error) {
	var set struct {
		Keys []struct {
			KeyType string `json:"kty"`
			Curve   string `json:"crv"`
			X       string `json:"x"`
			KeyID   string `json:"kid"`
		} `json:"keys"`
	}
	if failure := json.Unmarshal(document, &set); failure != nil {
		return nil, fmt.Errorf("not a JWKS: %w", failure)
	}
	if len(set.Keys) == 0 {
		return nil, errors.New("the JWKS holds no key")
	}
	verifying := &VerifyingKeys{keys: map[string]ed25519.PublicKey{}}
	for position, key := range set.Keys {
		if key.KeyType != "OKP" || key.Curve != "Ed25519" {
			return nil, fmt.Errorf("key %d is not an OKP Ed25519 key", position)
		}
		public, failure := base64.RawURLEncoding.DecodeString(key.X)
		if failure != nil || len(public) != ed25519.PublicKeySize {
			return nil, fmt.Errorf("key %d has no 32-byte x", position)
		}
		keyID := Thumbprint(public)
		if key.KeyID != "" && key.KeyID != keyID {
			return nil, fmt.Errorf("key %d declares kid %s but its RFC 7638 thumbprint is %s", position, key.KeyID, keyID)
		}
		verifying.keys[keyID] = public
	}
	return verifying, nil
}

func LoadVerifyingKeys(path string) (*VerifyingKeys, error) {
	document, failure := os.ReadFile(path)
	if failure != nil {
		return nil, fmt.Errorf("read the ledger keys: %w", failure)
	}
	keys, failure := ParseVerifyingKeys(document)
	if failure != nil {
		return nil, fmt.Errorf("%s: %w", path, failure)
	}
	return keys, nil
}

func (keys *VerifyingKeys) Count() int {
	return len(keys.keys)
}

func (keys *VerifyingKeys) Verify(keyID string, message []byte, signature string) error {
	public, known := keys.keys[keyID]
	if !known {
		return ErrUnknownKey
	}
	decoded, failure := base64.StdEncoding.DecodeString(signature)
	if failure != nil || len(decoded) != ed25519.SignatureSize {
		return ErrMalformedSignature
	}
	if !ed25519.Verify(public, message, decoded) {
		return ErrInvalidSignature
	}
	return nil
}

func CheckpointMessage(instance string, partition, sequence int64, hash string) ([]byte, error) {
	return canonicalBytes(map[string]any{
		"instance":  instance,
		"partition": json.Number(strconv.FormatInt(partition, 10)),
		"sequence":  json.Number(strconv.FormatInt(sequence, 10)),
		"hash":      hash,
	})
}
