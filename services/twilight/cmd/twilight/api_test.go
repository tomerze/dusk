package main

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/pem"
	"io"
	"log/slog"
	"math/big"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"testing"
	"time"

	"dusk/services/twilight/internal/config"
)

type authority struct {
	certificate *x509.Certificate
	key         *ecdsa.PrivateKey
}

func newAuthority(test *testing.T, name string) authority {
	key, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	template := &x509.Certificate{SerialNumber: big.NewInt(1), Subject: pkix.Name{CommonName: name}, NotBefore: time.Now().Add(-time.Hour), NotAfter: time.Now().Add(time.Hour),
		IsCA: true, BasicConstraintsValid: true, KeyUsage: x509.KeyUsageCertSign}
	encoded, failure := x509.CreateCertificate(rand.Reader, template, template, &key.PublicKey, key)
	if failure != nil {
		test.Fatal(failure)
	}
	certificate, _ := x509.ParseCertificate(encoded)
	return authority{certificate: certificate, key: key}
}

func (issuer authority) issue(test *testing.T, serial int64, usage x509.ExtKeyUsage, hosts []string, uris ...string) tls.Certificate {
	key, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	template := &x509.Certificate{SerialNumber: big.NewInt(serial), NotBefore: time.Now().Add(-time.Hour), NotAfter: time.Now().Add(time.Hour),
		KeyUsage: x509.KeyUsageDigitalSignature, ExtKeyUsage: []x509.ExtKeyUsage{usage}, DNSNames: hosts}
	for _, text := range uris {
		parsed, _ := url.Parse(text)
		template.URIs = append(template.URIs, parsed)
	}
	encoded, failure := x509.CreateCertificate(rand.Reader, template, issuer.certificate, &key.PublicKey, issuer.key)
	if failure != nil {
		test.Fatal(failure)
	}
	return tls.Certificate{Certificate: [][]byte{encoded}, PrivateKey: key}
}

func writePair(test *testing.T, pair tls.Certificate, certificatePath, keyPath string) {
	encodedKey, _ := x509.MarshalPKCS8PrivateKey(pair.PrivateKey)
	if failure := os.WriteFile(certificatePath, pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: pair.Certificate[0]}), 0o600); failure != nil {
		test.Fatal(failure)
	}
	if failure := os.WriteFile(keyPath, pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: encodedKey}), 0o600); failure != nil {
		test.Fatal(failure)
	}
}

func discard() *slog.Logger {
	return slog.New(slog.NewJSONHandler(io.Discard, nil))
}

func TestTheAPIListenerServesTLSAndVerifiesClientCertificates(test *testing.T) {
	internal, other := newAuthority(test, "internal"), newAuthority(test, "other")
	directory := test.TempDir()
	settings := config.Default()
	settings.Listen = "127.0.0.1:0"
	settings.TLS = config.TLS{Certificate: filepath.Join(directory, "tls.crt"), Key: filepath.Join(directory, "tls.key"), ClientCA: filepath.Join(directory, "client-ca.crt")}
	writePair(test, internal.issue(test, 2, x509.ExtKeyUsageServerAuth, []string{"twilight"}), settings.TLS.Certificate, settings.TLS.Key)
	if failure := os.WriteFile(settings.TLS.ClientCA, pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: internal.certificate.Raw}), 0o600); failure != nil {
		test.Fatal(failure)
	}
	listener, failure := apiListener(settings, discard())
	if failure != nil {
		test.Fatal(failure)
	}
	server := apiServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		if len(request.TLS.VerifiedChains) == 0 {
			io.WriteString(writer, "anonymous")
			return
		}
		io.WriteString(writer, request.TLS.VerifiedChains[0][0].URIs[0].String())
	}), discard())
	go server.Serve(listener)
	defer server.Close()

	roots := x509.NewCertPool()
	roots.AddCert(internal.certificate)
	call := func(certificates ...tls.Certificate) (string, error) {
		configuration := &tls.Config{RootCAs: roots, ServerName: "twilight", GetClientCertificate: func(*tls.CertificateRequestInfo) (*tls.Certificate, error) {
			if len(certificates) == 0 {
				return &tls.Certificate{}, nil
			}
			return &certificates[0], nil
		}}
		client := &http.Client{Timeout: 5 * time.Second, Transport: &http.Transport{TLSClientConfig: configuration}}
		response, failure := client.Get("https://" + listener.Addr().String() + "/")
		if failure != nil {
			return "", failure
		}
		defer response.Body.Close()
		body, _ := io.ReadAll(response.Body)
		return string(body), nil
	}
	if body, failure := call(); failure != nil || body != "anonymous" {
		test.Fatalf("without a client certificate: %q %v", body, failure)
	}
	if body, failure := call(internal.issue(test, 3, x509.ExtKeyUsageClientAuth, nil, "urn:dusk:principal:dawn-0")); failure != nil || body != "urn:dusk:principal:dawn-0" {
		test.Fatalf("with an internal client certificate: %q %v", body, failure)
	}
	if body, failure := call(other.issue(test, 4, x509.ExtKeyUsageClientAuth, nil, "urn:dusk:principal:dawn-0")); failure == nil {
		test.Fatalf("a client certificate from another CA was accepted: %q", body)
	}
}

func TestTheAPIListenerRefusesBrokenTLSSettings(test *testing.T) {
	directory := test.TempDir()
	internal := newAuthority(test, "internal")
	certificatePath, keyPath := filepath.Join(directory, "tls.crt"), filepath.Join(directory, "tls.key")
	writePair(test, internal.issue(test, 2, x509.ExtKeyUsageServerAuth, []string{"twilight"}), certificatePath, keyPath)
	notPEM := filepath.Join(directory, "not.pem")
	if failure := os.WriteFile(notPEM, []byte("not a certificate"), 0o600); failure != nil {
		test.Fatal(failure)
	}
	for name, settings := range map[string]config.TLS{
		"a missing certificate":         {Certificate: filepath.Join(directory, "missing.crt"), Key: keyPath},
		"a certificate that is not PEM": {Certificate: notPEM, Key: keyPath},
		"a missing client CA":           {Certificate: certificatePath, Key: keyPath, ClientCA: filepath.Join(directory, "missing-ca.crt")},
		"a client CA that is not PEM":   {Certificate: certificatePath, Key: keyPath, ClientCA: notPEM},
	} {
		current := config.Default()
		current.Listen = "127.0.0.1:0"
		current.TLS = settings
		if listener, failure := apiListener(current, discard()); failure == nil {
			listener.Close()
			test.Errorf("%s was accepted", name)
		}
	}
}

func TestARenewedCertificateIsServedAndABrokenOneIsNot(test *testing.T) {
	directory := test.TempDir()
	internal := newAuthority(test, "internal")
	files := &certificateFiles{certificate: filepath.Join(directory, "tls.crt"), key: filepath.Join(directory, "tls.key"), logger: discard()}
	writePair(test, internal.issue(test, 2, x509.ExtKeyUsageServerAuth, []string{"twilight"}), files.certificate, files.key)
	first, failure := files.current(nil)
	if failure != nil {
		test.Fatal(failure)
	}
	writePair(test, internal.issue(test, 3, x509.ExtKeyUsageServerAuth, []string{"twilight"}), files.certificate, files.key)
	later := time.Now().Add(time.Minute)
	for _, path := range []string{files.certificate, files.key} {
		if failure := os.Chtimes(path, later, later); failure != nil {
			test.Fatal(failure)
		}
	}
	if unchanged, _ := files.current(nil); unchanged != first {
		test.Fatal("the certificate was read again before 30 seconds passed")
	}
	files.checked = time.Time{}
	renewed, failure := files.current(nil)
	if failure != nil || renewed == first || renewed.Leaf.SerialNumber.Int64() != 3 {
		test.Fatalf("the renewed certificate was not loaded: %v", failure)
	}
	if failure := os.WriteFile(files.certificate, []byte("half written"), 0o600); failure != nil {
		test.Fatal(failure)
	}
	evenLater := later.Add(time.Minute)
	if failure := os.Chtimes(files.certificate, evenLater, evenLater); failure != nil {
		test.Fatal(failure)
	}
	files.checked = time.Time{}
	if kept, failure := files.current(nil); failure != nil || kept != renewed {
		test.Fatalf("a broken certificate replaced the one served: %v", failure)
	}
	if failure := os.Remove(files.key); failure != nil {
		test.Fatal(failure)
	}
	files.checked = time.Time{}
	if kept, failure := files.current(nil); failure != nil || kept != renewed {
		test.Fatalf("a missing key stopped the served certificate: %v", failure)
	}
}
