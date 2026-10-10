package api

import (
	"bytes"
	"context"
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"crypto/subtle"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"net/http"
	"net/url"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/coreos/go-oidc/v3/oidc"
	"golang.org/x/oauth2"
)

const (
	loginLifetime       = 10 * time.Minute
	discoveryTimeout    = 10 * time.Second
	exchangeTimeout     = 15 * time.Second
	maximumReturnLength = 2048
)

type OIDCSettings struct {
	Issuer       string
	ClientID     string
	ClientSecret string
	RedirectURL  string
	Scopes       []string
	RoleClaim    string
	RoleMap      map[string]Role
	HTTPClient   *http.Client
}

type oidcClient struct {
	settings OIDCSettings
	mutex    sync.Mutex
	provider *oidc.Provider
	key      cipher.AEAD
}

type pendingLogin struct {
	State     string    `json:"state"`
	Nonce     string    `json:"nonce"`
	Verifier  string    `json:"verifier"`
	ReturnTo  string    `json:"return_to"`
	ExpiresAt time.Time `json:"expires_at"`
}

const loginSealContext = "twilight_login"

func (client *oidcClient) sealer(operation context.Context, sessions SessionStore) (cipher.AEAD, error) {
	client.mutex.Lock()
	defer client.mutex.Unlock()
	if client.key != nil {
		return client.key, nil
	}
	key, failure := sessions.LoginKey(operation)
	if failure != nil {
		return nil, fmt.Errorf("read the login key: %w", failure)
	}
	block, failure := aes.NewCipher(key)
	if failure != nil {
		return nil, fmt.Errorf("the login key: %w", failure)
	}
	sealer, failure := cipher.NewGCM(block)
	if failure != nil {
		return nil, failure
	}
	client.key = sealer
	return sealer, nil
}

func sealLogin(sealer cipher.AEAD, login pendingLogin) (string, error) {
	var plain bytes.Buffer
	encoder := json.NewEncoder(&plain)
	encoder.SetEscapeHTML(false)
	if failure := encoder.Encode(login); failure != nil {
		return "", failure
	}
	nonce := make([]byte, sealer.NonceSize())
	if _, failure := rand.Read(nonce); failure != nil {
		return "", failure
	}
	return base64.RawURLEncoding.EncodeToString(sealer.Seal(nonce, nonce, plain.Bytes(), []byte(loginSealContext))), nil
}

func openLogin(sealer cipher.AEAD, sealed string) (pendingLogin, error) {
	var login pendingLogin
	raw, failure := base64.RawURLEncoding.DecodeString(sealed)
	if failure != nil || len(raw) < sealer.NonceSize() {
		return login, errors.New("the login cookie does not decode")
	}
	plain, failure := sealer.Open(nil, raw[:sealer.NonceSize()], raw[sealer.NonceSize():], []byte(loginSealContext))
	if failure != nil {
		return login, errors.New("the login cookie was not sealed by twilight")
	}
	return login, json.Unmarshal(plain, &login)
}

func (client *oidcClient) context(operation context.Context) context.Context {
	return oidc.ClientContext(operation, client.settings.HTTPClient)
}

func (client *oidcClient) discover(operation context.Context) (*oidc.Provider, error) {
	client.mutex.Lock()
	defer client.mutex.Unlock()
	if client.provider != nil {
		return client.provider, nil
	}
	discovering, cancel := context.WithTimeout(client.context(operation), discoveryTimeout)
	defer cancel()
	provider, failure := oidc.NewProvider(discovering, client.settings.Issuer)
	if failure != nil {
		return nil, failure
	}
	client.provider = provider
	return provider, nil
}

func (client *oidcClient) configuration(provider *oidc.Provider) oauth2.Config {
	scopes := []string{oidc.ScopeOpenID}
	for _, scope := range client.settings.Scopes {
		if scope != oidc.ScopeOpenID {
			scopes = append(scopes, scope)
		}
	}
	return oauth2.Config{ClientID: client.settings.ClientID, ClientSecret: client.settings.ClientSecret, Endpoint: provider.Endpoint(), RedirectURL: client.settings.RedirectURL, Scopes: scopes}
}

func returnTarget(call *exchange) (string, error) {
	target := call.request.URL.Query().Get("return_to")
	if target == "" {
		return "/", nil
	}
	parsed, failure := url.Parse(target)
	if failure != nil || len(target) > maximumReturnLength || !strings.HasPrefix(target, "/") || strings.HasPrefix(target, "//") || strings.Contains(target, `\`) || parsed.Host != "" || parsed.Scheme != "" {
		return "", invalid("return_to must be a path on this site")
	}
	return target, nil
}

func loopback(remote string) bool {
	host, _, failure := net.SplitHostPort(remote)
	if failure != nil {
		return false
	}
	address := net.ParseIP(host)
	return address != nil && address.IsLoopback()
}

func (server *Server) login(call *exchange) error {
	returnTo, failure := returnTarget(call)
	if failure != nil {
		return failure
	}
	if server.Development {
		if !loopback(call.request.RemoteAddr) {
			server.Logger.Warn("a development login from a remote address was refused", "remote_address", call.request.RemoteAddr)
			return newProblem(http.StatusForbidden, "forbidden", "development logins are only accepted from a loopback address")
		}
		name := "development"
		return server.startSession(call, "dev", &name, RoleAdmin, AuthenticationDevelopment, returnTo)
	}
	if server.oidc == nil {
		return newProblem(http.StatusNotFound, "login_unavailable", "this twilight has no OIDC issuer configured; use an API token")
	}
	provider, failure := server.oidc.discover(call.request.Context())
	if failure != nil {
		server.Logger.Error("the OIDC issuer could not be discovered", "issuer", server.oidc.settings.Issuer, "error", failure, "request_id", call.requestID)
		return newProblem(http.StatusServiceUnavailable, "issuer_unavailable", "the identity provider could not be reached; try again shortly").with("retry_after_seconds", 5)
	}
	sealer, failure := server.oidc.sealer(call.request.Context(), server.Sessions)
	if failure != nil {
		return failure
	}
	state, nonce, verifier := randomSecret(), randomSecret(), oauth2.GenerateVerifier()
	sealed, failure := sealLogin(sealer, pendingLogin{State: state, Nonce: nonce, Verifier: verifier, ReturnTo: returnTo, ExpiresAt: server.Now().Add(loginLifetime)})
	if failure != nil {
		return failure
	}
	configuration := server.oidc.configuration(provider)
	http.SetCookie(call.writer, server.cookie(LoginCookie, sealed, loginLifetime, true, CallbackPath))
	http.Redirect(call.writer, call.request, configuration.AuthCodeURL(state, oauth2.S256ChallengeOption(verifier), oidc.Nonce(nonce)), http.StatusFound)
	return nil
}

func (server *Server) loginFailed(call *exchange, reason string, failure error) *Problem {
	authenticationFailures.WithLabelValues("oidc").Inc()
	server.Logger.Warn("an OIDC login failed", "reason", reason, "error", failure, "remote_address", call.request.RemoteAddr, "request_id", call.requestID)
	return newProblem(http.StatusUnauthorized, "login_failed", reason).with("login", LoginPath)
}

func (server *Server) callback(call *exchange) error {
	if server.oidc == nil {
		return newProblem(http.StatusNotFound, "login_unavailable", "this twilight has no OIDC issuer configured")
	}
	query := call.request.URL.Query()
	http.SetCookie(call.writer, server.cookie(LoginCookie, "", -1, true, CallbackPath))
	if refused := query.Get("error"); refused != "" {
		return server.loginFailed(call, "the identity provider refused the login: "+refused, errors.New(query.Get("error_description")))
	}
	state := query.Get("state")
	cookie, failure := call.request.Cookie(LoginCookie)
	if state == "" || failure != nil {
		return server.loginFailed(call, "the login did not start in this browser; start it again", failure)
	}
	sealer, failure := server.oidc.sealer(call.request.Context(), server.Sessions)
	if failure != nil {
		return failure
	}
	login, failure := openLogin(sealer, cookie.Value)
	if failure != nil || subtle.ConstantTimeCompare([]byte(state), []byte(login.State)) != 1 {
		return server.loginFailed(call, "the login did not start in this browser; start it again", failure)
	}
	if !server.Now().Before(login.ExpiresAt) {
		return server.loginFailed(call, "the login expired; start it again", nil)
	}
	provider, failure := server.oidc.discover(call.request.Context())
	if failure != nil {
		return newProblem(http.StatusServiceUnavailable, "issuer_unavailable", "the identity provider could not be reached; try again shortly").with("retry_after_seconds", 5)
	}
	exchanging, cancel := context.WithTimeout(context.WithValue(call.request.Context(), oauth2.HTTPClient, server.oidc.settings.HTTPClient), exchangeTimeout)
	defer cancel()
	configuration := server.oidc.configuration(provider)
	granted, failure := configuration.Exchange(exchanging, query.Get("code"), oauth2.VerifierOption(login.Verifier))
	if failure != nil {
		return server.loginFailed(call, "the identity provider did not accept the login code", failure)
	}
	raw, found := granted.Extra("id_token").(string)
	if !found || raw == "" {
		return server.loginFailed(call, "the identity provider returned no ID token", nil)
	}
	identity, failure := provider.Verifier(&oidc.Config{ClientID: server.oidc.settings.ClientID}).Verify(server.oidc.context(exchanging), raw)
	if failure != nil {
		return server.loginFailed(call, "the ID token did not verify", failure)
	}
	if subtle.ConstantTimeCompare([]byte(identity.Nonce), []byte(login.Nonce)) != 1 {
		return server.loginFailed(call, "the ID token was not issued for this login", nil)
	}
	var claims map[string]any
	if failure := identity.Claims(&claims); failure != nil {
		return server.loginFailed(call, "the ID token's claims could not be read", failure)
	}
	values := claimValues(claims, server.oidc.settings.RoleClaim)
	role := RolePublic
	for _, value := range values {
		if mapped, found := server.oidc.settings.RoleMap[value]; found && mapped > role {
			role = mapped
		}
	}
	if role == RolePublic {
		authenticationFailures.WithLabelValues("no_role").Inc()
		server.Logger.Warn("an OIDC login has no role", "principal", identity.Subject, "claim", server.oidc.settings.RoleClaim, "values", values, "remote_address", call.request.RemoteAddr)
		return newProblem(http.StatusForbidden, "no_role", fmt.Sprintf("you are signed in as %s, but none of your %s maps to a twilight role; ask an administrator to add one to oidc.role_map", identity.Subject, server.oidc.settings.RoleClaim))
	}
	return server.startSession(call, identity.Subject, displayName(claims), role, AuthenticationOIDC, login.ReturnTo)
}

func claimValues(claims map[string]any, path string) []string {
	current, whole := claims[path]
	if !whole {
		current = claims
		for part := range strings.SplitSeq(path, ".") {
			object, isObject := current.(map[string]any)
			if !isObject {
				return nil
			}
			current = object[part]
		}
	}
	var values []string
	switch typed := current.(type) {
	case string:
		values = append(values, typed)
	case []any:
		for _, item := range typed {
			if text, isText := item.(string); isText {
				values = append(values, text)
			}
		}
	}
	sort.Strings(values)
	return values
}

func displayName(claims map[string]any) *string {
	for _, claim := range []string{"name", "preferred_username", "email"} {
		if text, isText := claims[claim].(string); isText && strings.TrimSpace(text) != "" {
			return &text
		}
	}
	return nil
}
