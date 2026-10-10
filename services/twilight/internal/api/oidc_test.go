package api

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"sync"
	"testing"
	"time"
)

type grant struct {
	challenge   string
	redirectURI string
	claims      map[string]any
}

type fakeIssuer struct {
	server   *httptest.Server
	key      *ecdsa.PrivateKey
	clientID string
	mutex    sync.Mutex
	grants   map[string]grant
	refusals []string
}

func newFakeIssuer(test *testing.T) *fakeIssuer {
	key, failure := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if failure != nil {
		test.Fatal(failure)
	}
	issuer := &fakeIssuer{key: key, clientID: "twilight", grants: map[string]grant{}}
	mux := http.NewServeMux()
	mux.HandleFunc("GET /.well-known/openid-configuration", func(writer http.ResponseWriter, _ *http.Request) {
		_ = json.NewEncoder(writer).Encode(map[string]any{
			"issuer": issuer.server.URL, "authorization_endpoint": issuer.server.URL + "/authorize", "token_endpoint": issuer.server.URL + "/token",
			"jwks_uri": issuer.server.URL + "/keys", "response_types_supported": []string{"code"}, "subject_types_supported": []string{"public"},
			"id_token_signing_alg_values_supported": []string{"ES256"}, "code_challenge_methods_supported": []string{"S256"},
		})
	})
	mux.HandleFunc("GET /keys", func(writer http.ResponseWriter, _ *http.Request) {
		_ = json.NewEncoder(writer).Encode(map[string]any{"keys": []map[string]string{{
			"kty": "EC", "crv": "P-256", "kid": "test", "alg": "ES256", "use": "sig",
			"x": base64.RawURLEncoding.EncodeToString(key.X.FillBytes(make([]byte, 32))), "y": base64.RawURLEncoding.EncodeToString(key.Y.FillBytes(make([]byte, 32))),
		}}})
	})
	mux.HandleFunc("POST /token", func(writer http.ResponseWriter, request *http.Request) {
		_ = request.ParseForm()
		issuer.mutex.Lock()
		granted, found := issuer.grants[request.PostForm.Get("code")]
		delete(issuer.grants, request.PostForm.Get("code"))
		issuer.mutex.Unlock()
		sum := sha256.Sum256([]byte(request.PostForm.Get("code_verifier")))
		refuse := func(reason string) {
			issuer.mutex.Lock()
			issuer.refusals = append(issuer.refusals, reason)
			issuer.mutex.Unlock()
			writer.Header().Set("Content-Type", "application/json")
			writer.WriteHeader(http.StatusBadRequest)
			_, _ = writer.Write([]byte(`{"error":"invalid_grant"}`))
		}
		switch {
		case !found:
			refuse("unknown code")
			return
		case request.PostForm.Get("grant_type") != "authorization_code":
			refuse("grant type")
			return
		case base64.RawURLEncoding.EncodeToString(sum[:]) != granted.challenge:
			refuse("pkce")
			return
		case request.PostForm.Get("redirect_uri") != granted.redirectURI:
			refuse("redirect uri")
			return
		}
		writer.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(writer).Encode(map[string]any{"access_token": "opaque", "token_type": "Bearer", "expires_in": 300, "id_token": issuer.sign(granted.claims)})
	})
	issuer.server = httptest.NewServer(mux)
	test.Cleanup(issuer.server.Close)
	return issuer
}

func (issuer *fakeIssuer) sign(claims map[string]any) string {
	header, _ := json.Marshal(map[string]string{"alg": "ES256", "kid": "test", "typ": "JWT"})
	payload, _ := json.Marshal(claims)
	signed := base64.RawURLEncoding.EncodeToString(header) + "." + base64.RawURLEncoding.EncodeToString(payload)
	sum := sha256.Sum256([]byte(signed))
	first, second, _ := ecdsa.Sign(rand.Reader, issuer.key, sum[:])
	signature := append(first.FillBytes(make([]byte, 32)), second.FillBytes(make([]byte, 32))...)
	return signed + "." + base64.RawURLEncoding.EncodeToString(signature)
}

func (issuer *fakeIssuer) claims(subject, nonce string, extra map[string]any) map[string]any {
	now := time.Now()
	claims := map[string]any{"iss": issuer.server.URL, "sub": subject, "aud": issuer.clientID, "iat": now.Unix(), "exp": now.Add(5 * time.Minute).Unix(), "nonce": nonce}
	for key, value := range extra {
		claims[key] = value
	}
	return claims
}

func oidcHarness(test *testing.T, issuer *fakeIssuer, roleClaim string) *harness {
	return newHarness(test, func(options *Options) {
		options.OIDC = &OIDCSettings{
			Issuer: issuer.server.URL, ClientID: issuer.clientID, RedirectURL: "https://twilight.example/api/v1/auth/callback", Scopes: []string{"openid", "profile", "groups"},
			RoleClaim: roleClaim, RoleMap: map[string]Role{"fleet-viewers": RoleViewer, "fleet-operators": RoleOperator, "fleet-admins": RoleAdmin},
			HTTPClient: &http.Client{Timeout: 5 * time.Second},
		}
	})
}

type started struct {
	query  url.Values
	cookie *http.Cookie
}

func (current *harness) startLogin(returnTo string) started {
	current.test.Helper()
	target := LoginPath
	if returnTo != "" {
		target += "?return_to=" + url.QueryEscape(returnTo)
	}
	result := current.call(http.MethodGet, target, nil, "")
	if result.Code != http.StatusFound {
		current.test.Fatalf("login answered %d %s", result.Code, result.Body.String())
	}
	location, _ := url.Parse(result.Header().Get("Location"))
	login := started{query: location.Query()}
	for _, set := range result.Result().Cookies() {
		if set.Name == LoginCookie {
			login.cookie = set
		}
	}
	if login.cookie == nil || !login.cookie.HttpOnly || login.cookie.Path != CallbackPath || strings.Contains(login.cookie.Value, login.query.Get("state")) {
		current.test.Fatalf("login cookie %+v", login.cookie)
	}
	return login
}

func (issuer *fakeIssuer) authorize(login started, claims map[string]any) string {
	code := randomSecret()
	issuer.mutex.Lock()
	issuer.grants[code] = grant{challenge: login.query.Get("code_challenge"), redirectURI: login.query.Get("redirect_uri"), claims: claims}
	issuer.mutex.Unlock()
	return CallbackPath + "?code=" + url.QueryEscape(code) + "&state=" + url.QueryEscape(login.query.Get("state"))
}

func (current *harness) finishLogin(target string, cookie *http.Cookie) response {
	request := newRequest(http.MethodGet, target, nil, "")
	if cookie != nil {
		request.AddCookie(cookie)
	}
	return current.do(request)
}

func TestOIDCLoginWithPKCE(test *testing.T) {
	issuer := newFakeIssuer(test)
	current := oidcHarness(test, issuer, "groups")
	login := current.startLogin("/campaigns?status=running")
	query := login.query
	if query.Get("code_challenge_method") != "S256" || len(query.Get("code_challenge")) != 43 || query.Get("client_id") != "twilight" ||
		query.Get("response_type") != "code" || query.Get("nonce") == "" || query.Get("scope") != "openid profile groups" || query.Get("redirect_uri") != "https://twilight.example/api/v1/auth/callback" {
		test.Fatalf("authorization request %v", query)
	}
	target := issuer.authorize(login, issuer.claims("user-123", query.Get("nonce"), map[string]any{"groups": []any{"unrelated", "fleet-operators", "fleet-admins"}, "name": "Ada Lovelace"}))
	finished := current.finishLogin(target, login.cookie)
	if finished.Code != http.StatusSeeOther || finished.Header().Get("Location") != "/campaigns?status=running" {
		test.Fatalf("callback %d %s %s", finished.Code, finished.Header(), finished.Body.String())
	}
	var session *http.Cookie
	for _, set := range finished.Result().Cookies() {
		if set.Name == SessionCookie {
			session = set
		}
	}
	if session == nil || !session.Secure || !session.HttpOnly || session.SameSite != http.SameSiteLaxMode || session.MaxAge != int((12*time.Hour).Seconds()) {
		test.Fatalf("session cookie %+v", session)
	}
	read := newRequest(http.MethodGet, "/api/v1/me", nil, "")
	read.AddCookie(session)
	me := current.do(read)
	if field(me, "subject") != "user-123" || field(me, "role") != "admin" || field(me, "name") != "Ada Lovelace" || field(me, "authentication") != "oidc" {
		test.Fatalf("%s", me.Body.String())
	}
	if replayed := current.finishLogin(target, login.cookie); replayed.Code != http.StatusUnauthorized || errorCode(replayed) != "login_failed" {
		test.Fatalf("a replayed callback answered %d %s", replayed.Code, replayed.Body.String())
	}
	if !strings.Contains(current.logs.String(), `"msg":"login session started"`) {
		test.Fatal("the login was not logged")
	}
}

func TestOIDCLoginRefusals(test *testing.T) {
	issuer := newFakeIssuer(test)
	current := oidcHarness(test, issuer, "groups")

	login := current.startLogin("")
	unmapped := current.finishLogin(issuer.authorize(login, issuer.claims("user-9", login.query.Get("nonce"), map[string]any{"groups": []any{"contractors"}})), login.cookie)
	if unmapped.Code != http.StatusForbidden || errorCode(unmapped) != "no_role" || !strings.Contains(current.logs.String(), `"msg":"an OIDC login has no role"`) {
		test.Fatalf("a login without a mapped role: %d %s", unmapped.Code, unmapped.Body.String())
	}
	if len(current.sessions.sessions) != 0 {
		test.Fatal("a login without a role created a session")
	}

	login = current.startLogin("")
	if other := current.finishLogin(issuer.authorize(login, issuer.claims("user-1", login.query.Get("nonce"), map[string]any{"groups": "fleet-viewers"})), &http.Cookie{Name: LoginCookie, Value: randomSecret()}); other.Code != http.StatusUnauthorized {
		test.Fatalf("a callback in another browser answered %d", other.Code)
	}
	login = current.startLogin("")
	if missing := current.finishLogin(issuer.authorize(login, issuer.claims("user-1", login.query.Get("nonce"), nil)), nil); missing.Code != http.StatusUnauthorized {
		test.Fatalf("a callback without the login cookie answered %d", missing.Code)
	}

	login = current.startLogin("")
	wrongNonce := current.finishLogin(issuer.authorize(login, issuer.claims("user-1", "another nonce", map[string]any{"groups": []any{"fleet-viewers"}})), login.cookie)
	if wrongNonce.Code != http.StatusUnauthorized || !strings.Contains(wrongNonce.Body.String(), "not issued for this login") {
		test.Fatalf("a token for another login answered %d %s", wrongNonce.Code, wrongNonce.Body.String())
	}

	login = current.startLogin("")
	forged := login
	forged.query = url.Values{"state": {login.query.Get("state")}, "redirect_uri": {login.query.Get("redirect_uri")}, "code_challenge": {base64.RawURLEncoding.EncodeToString(make([]byte, 32))}}
	if stolen := current.finishLogin(issuer.authorize(forged, issuer.claims("user-1", login.query.Get("nonce"), nil)), login.cookie); stolen.Code != http.StatusUnauthorized {
		test.Fatalf("a code bound to another verifier answered %d", stolen.Code)
	}
	if !strings.Contains(fmt.Sprint(issuer.refusals), "pkce") {
		test.Fatalf("the issuer did not refuse the verifier: %v", issuer.refusals)
	}

	login = current.startLogin("")
	expired := issuer.claims("user-1", login.query.Get("nonce"), map[string]any{"groups": []any{"fleet-admins"}})
	expired["exp"] = time.Now().Add(-time.Hour).Unix()
	if result := current.finishLogin(issuer.authorize(login, expired), login.cookie); result.Code != http.StatusUnauthorized || !strings.Contains(result.Body.String(), "did not verify") {
		test.Fatalf("an expired ID token answered %d %s", result.Code, result.Body.String())
	}

	login = current.startLogin("")
	foreign := issuer.claims("user-1", login.query.Get("nonce"), map[string]any{"groups": []any{"fleet-admins"}})
	foreign["aud"] = "another-client"
	if result := current.finishLogin(issuer.authorize(login, foreign), login.cookie); result.Code != http.StatusUnauthorized {
		test.Fatalf("an ID token for another client answered %d", result.Code)
	}

	first, second := current.startLogin(""), current.startLogin("")
	swapped := current.finishLogin(issuer.authorize(first, issuer.claims("user-1", first.query.Get("nonce"), map[string]any{"groups": []any{"fleet-admins"}})), second.cookie)
	if swapped.Code != http.StatusUnauthorized || !strings.Contains(swapped.Body.String(), "did not start in this browser") {
		test.Fatalf("a callback with another login's cookie answered %d %s", swapped.Code, swapped.Body.String())
	}
	login = current.startLogin("")
	tampered := *login.cookie
	middle, replacement := len(tampered.Value)/2, "A"
	if tampered.Value[middle] == 'A' {
		replacement = "B"
	}
	tampered.Value = tampered.Value[:middle] + replacement + tampered.Value[middle+1:]
	if result := current.finishLogin(issuer.authorize(login, issuer.claims("user-1", login.query.Get("nonce"), map[string]any{"groups": []any{"fleet-admins"}})), &tampered); result.Code != http.StatusUnauthorized {
		test.Fatalf("a login cookie changed in the browser answered %d %s", result.Code, result.Body.String())
	}
	login = current.startLogin("")
	current.now = current.now.Add(loginLifetime)
	late := current.finishLogin(issuer.authorize(login, issuer.claims("user-1", login.query.Get("nonce"), map[string]any{"groups": []any{"fleet-admins"}})), login.cookie)
	current.now = current.now.Add(-loginLifetime)
	if late.Code != http.StatusUnauthorized || !strings.Contains(late.Body.String(), "expired") {
		test.Fatalf("a login finished after its lifetime answered %d %s", late.Code, late.Body.String())
	}

	denied := current.finishLogin(CallbackPath+"?error=access_denied&error_description=no", nil)
	if denied.Code != http.StatusUnauthorized || !strings.Contains(denied.Body.String(), "access_denied") {
		test.Fatalf("a refusal by the issuer answered %d %s", denied.Code, denied.Body.String())
	}
	if len(current.sessions.sessions) != 0 {
		test.Fatalf("%d sessions after refused logins", len(current.sessions.sessions))
	}
}

func TestOIDCRoleFromANestedClaim(test *testing.T) {
	issuer := newFakeIssuer(test)
	current := oidcHarness(test, issuer, "realm_access.roles")
	login := current.startLogin("")
	finished := current.finishLogin(issuer.authorize(login, issuer.claims("user-5", login.query.Get("nonce"), map[string]any{"realm_access": map[string]any{"roles": []any{"fleet-viewers"}}})), login.cookie)
	if finished.Code != http.StatusSeeOther || finished.Header().Get("Location") != "/" {
		test.Fatalf("%d %s", finished.Code, finished.Body.String())
	}
	claims := map[string]any{"https://fleet.example/roles": []any{"b", "a", 3}, "groups": "solo", "nested": map[string]any{"deeper": map[string]any{"value": []any{"x"}}}}
	for path, want := range map[string]string{"https://fleet.example/roles": "[a b]", "groups": "[solo]", "nested.deeper.value": "[x]", "nested.absent": "[]", "groups.child": "[]"} {
		if got := fmt.Sprint(claimValues(claims, path)); got != want {
			test.Errorf("%s: %s, want %s", path, got, want)
		}
	}
}

func TestAnUnreachableIssuer(test *testing.T) {
	closed := httptest.NewServer(http.NotFoundHandler())
	closed.Close()
	current := newHarness(test, func(options *Options) {
		options.OIDC = &OIDCSettings{Issuer: closed.URL, ClientID: "twilight", RedirectURL: "https://twilight.example/api/v1/auth/callback", RoleClaim: "groups",
			RoleMap: map[string]Role{"x": RoleViewer}, HTTPClient: &http.Client{Timeout: time.Second}}
	})
	result := current.call(http.MethodGet, LoginPath, nil, "")
	if result.Code != http.StatusServiceUnavailable || errorCode(result) != "issuer_unavailable" || result.Header().Get("Retry-After") != "5" {
		test.Fatalf("%d %s", result.Code, result.Body.String())
	}
}

func TestLoginsInProgressHoldNoServerState(test *testing.T) {
	issuer := newFakeIssuer(test)
	current := oidcHarness(test, issuer, "groups")
	for range 200 {
		current.startLogin("/campaigns")
	}
	if len(current.sessions.sessions) != 0 {
		test.Fatal("starting a login stored something")
	}
}
