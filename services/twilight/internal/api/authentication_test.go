package api

import (
	"crypto/tls"
	"crypto/x509"
	"net/http"
	"net/url"
	"strings"
	"testing"
	"time"
)

func TestCallersWithoutCredentialsAreRefused(test *testing.T) {
	current := newHarness(test, nil)
	refused := current.call(http.MethodGet, "/api/v1/me", nil, "")
	if refused.Code != http.StatusUnauthorized || errorCode(refused) != "unauthenticated" || refused.Header().Get("WWW-Authenticate") != `Bearer realm="twilight"` {
		test.Fatalf("%d %s", refused.Code, refused.Body.String())
	}
	if _, offered := errorDetails(refused)["login"]; offered {
		test.Fatal("a login was offered without OIDC or development mode")
	}
	withLogin := newHarness(test, func(options *Options) { options.OIDC = &OIDCSettings{Issuer: "https://issuer.example"} })
	if offered := errorDetails(withLogin.call(http.MethodGet, "/api/v1/me", nil, ""))["login"]; offered != LoginPath {
		test.Fatalf("login %v", offered)
	}
	for _, header := range []string{"Basic dXNlcjpwYXNz", "Bearer", "Bearer ", "twilight_admin"} {
		request := newRequest(http.MethodGet, "/api/v1/me", nil, "")
		request.Header.Set("Authorization", header)
		if result := current.do(request); result.Code != http.StatusUnauthorized {
			test.Errorf("Authorization %q answered %d", header, result.Code)
		}
	}
	for _, token := range []string{"twilight_unknown", revokedToken} {
		if result := current.call(http.MethodGet, "/api/v1/me", nil, token); result.Code != http.StatusUnauthorized || !strings.Contains(result.Body.String(), "unknown or revoked") {
			test.Errorf("token %s answered %d %s", token, result.Code, result.Body.String())
		}
	}
	if !strings.Contains(current.logs.String(), `"msg":"api request not authenticated"`) {
		test.Fatal("refusals are not logged")
	}
}

func TestPublicRoutesNeedNoCredentials(test *testing.T) {
	current := newHarness(test, nil)
	if document := current.call(http.MethodGet, "/api/openapi.json", nil, ""); document.Code != http.StatusOK {
		test.Fatalf("the OpenAPI document answered %d", document.Code)
	}
	if login := current.call(http.MethodGet, LoginPath, nil, ""); login.Code != http.StatusNotFound || errorCode(login) != "login_unavailable" {
		test.Fatalf("login without OIDC %d %s", login.Code, login.Body.String())
	}
}

func TestTheCallerIsDescribed(test *testing.T) {
	current := newHarness(test, nil)
	me := current.call(http.MethodGet, "/api/v1/me", nil, operatorToken)
	if me.Code != http.StatusOK || field(me, "subject") != "token:00000000-0000-7000-8000-000000000002" || field(me, "role") != "operator" ||
		field(me, "authentication") != "token" || field(me, "name") != "release automation" || field(me, "expires_at") != nil {
		test.Fatalf("%s", me.Body.String())
	}
	if me.Header().Get("Cache-Control") != "no-store" || me.Header().Get("X-Request-Id") == "" {
		test.Fatalf("headers %s", me.Header())
	}
}

func roleRequired(current *harness, method, path string) Role {
	for _, candidate := range current.server.routes {
		if candidate.Method == method && candidate.Path == path {
			return candidate.Role
		}
	}
	return RolePublic
}

func TestEveryRouteEnforcesItsRole(test *testing.T) {
	current := newHarness(test, nil)
	concrete := strings.NewReplacer("{device}", deviceID, "{installation}", installationID, "{id}", campaignIdentifier.String())
	tokensByRole := map[Role]string{RoleViewer: viewerToken, RoleOperator: operatorToken, RoleAdmin: adminToken}
	for _, current := range current.server.routes {
		if current.Role == RolePublic {
			continue
		}
		path := concrete.Replace(current.Path)
		if strings.HasPrefix(current.Path, "/api/v1/alerts/{id}") {
			path = strings.Replace(current.Path, "{id}", "7", 1)
		}
		harness := newHarness(test, nil)
		if refused := harness.call(current.Method, path, nil, ""); refused.Code != http.StatusUnauthorized {
			test.Errorf("%s without credentials answered %d", current.pattern(), refused.Code)
		}
		for role, token := range tokensByRole {
			result := harness.call(current.Method, path, map[string]any{}, token)
			switch {
			case role < current.Role && (result.Code != http.StatusForbidden || errorCode(result) != "forbidden"):
				test.Errorf("%s as %s answered %d, not 403", current.pattern(), role, result.Code)
			case role >= current.Role && (result.Code == http.StatusForbidden || result.Code == http.StatusUnauthorized):
				test.Errorf("%s as %s answered %d: %s", current.pattern(), role, result.Code, result.Body.String())
			}
		}
	}
	if roleRequired(current, http.MethodPost, "/api/v1/campaigns/{id}/start") != RoleOperator || roleRequired(current, http.MethodGet, "/api/v1/stream") != RoleViewer {
		test.Fatal("the route table changed under this test")
	}
}

func TestRetiringAndRevokingNeedAnAdministrator(test *testing.T) {
	current := newHarness(test, nil)
	target := "/api/v1/nodes/" + deviceID + "/" + installationID + "/lifecycle"
	for _, lifecycle := range []string{"retired", "revoked"} {
		refused := current.call(http.MethodPost, target, map[string]any{"lifecycle": lifecycle, "reason": "stolen"}, operatorToken)
		if refused.Code != http.StatusForbidden || !strings.Contains(refused.Body.String(), "admin role") {
			test.Fatalf("%s as operator: %d %s", lifecycle, refused.Code, refused.Body.String())
		}
		if allowed := current.call(http.MethodPost, target, map[string]any{"lifecycle": lifecycle, "reason": "stolen"}, adminToken); allowed.Code != http.StatusOK {
			test.Fatalf("%s as admin: %d %s", lifecycle, allowed.Code, allowed.Body.String())
		}
	}
	for _, lifecycle := range []string{"active", "quarantined"} {
		refused := current.call(http.MethodPost, target, map[string]any{"lifecycle": lifecycle, "reason": "found it"}, operatorToken)
		if refused.Code != http.StatusForbidden || !strings.Contains(refused.Body.String(), "the node is revoked") || !strings.Contains(refused.Body.String(), "you have operator") {
			test.Fatalf("an operator moved a revoked node to %s: %d %s", lifecycle, refused.Code, refused.Body.String())
		}
	}
	if arguments := current.backend.last("SetLifecycle"); arguments[4] != false {
		test.Fatalf("an operator's call reached the backend as an administrator's: %v", arguments)
	}
	if restored := current.call(http.MethodPost, target, map[string]any{"lifecycle": "active", "reason": "recovered"}, adminToken); restored.Code != http.StatusOK {
		test.Fatalf("an administrator restores a revoked node: %d %s", restored.Code, restored.Body.String())
	}
	if quarantined := current.call(http.MethodPost, target, map[string]any{"lifecycle": "quarantined", "reason": "suspicious"}, operatorToken); quarantined.Code != http.StatusOK {
		test.Fatalf("quarantine as operator: %d %s", quarantined.Code, quarantined.Body.String())
	}
	arguments := current.backend.last("SetLifecycle")
	if arguments[1] != "quarantined" || arguments[2] != "suspicious" || arguments[3] != "token:00000000-0000-7000-8000-000000000002" || arguments[4] != false {
		test.Fatalf("SetLifecycle %v", arguments)
	}
}

func certificateRequest(uris ...string) *http.Request {
	certificate := &x509.Certificate{}
	for _, text := range uris {
		parsed, _ := url.Parse(text)
		certificate.URIs = append(certificate.URIs, parsed)
	}
	request := newRequest(http.MethodGet, "/api/v1/me", nil, "")
	request.TLS = &tls.ConnectionState{VerifiedChains: [][]*x509.Certificate{{certificate}}}
	return request
}

func TestServiceCallersAuthenticateWithCertificates(test *testing.T) {
	current := newHarness(test, nil)
	me := current.do(certificateRequest("urn:dusk:principal:automation-7"))
	if me.Code != http.StatusOK || field(me, "subject") != "automation-7" || field(me, "role") != "operator" || field(me, "authentication") != "mtls" {
		test.Fatalf("%d %s", me.Code, me.Body.String())
	}
	cases := map[string][]string{
		"no role":            {"urn:dusk:principal:dawn-0"},
		"no principal":       {"urn:example:other"},
		"several principals": {"urn:dusk:principal:automation-1", "urn:dusk:principal:automation-2"},
		"a node certificate": {"urn:dusk:principal:automation-1", "urn:dusk:device:" + deviceID},
	}
	for name, uris := range cases {
		if refused := current.do(certificateRequest(uris...)); refused.Code != http.StatusForbidden {
			test.Errorf("%s answered %d", name, refused.Code)
		}
	}
	unverified := newRequest(http.MethodGet, "/api/v1/me", nil, "")
	unverified.TLS = &tls.ConnectionState{PeerCertificates: []*x509.Certificate{{}}}
	if refused := current.do(unverified); refused.Code != http.StatusUnauthorized {
		test.Fatalf("an unverified certificate answered %d", refused.Code)
	}
	roles := PrincipalRoles{"dawn-*": RoleViewer, "dawn-0": RoleAdmin}
	if roles.RoleOf("dawn-0") != RoleAdmin || roles.RoleOf("dawn-1") != RoleViewer || roles.RoleOf("twilight-0") != RolePublic {
		test.Fatal("principal patterns do not grant the highest matching role")
	}
}

func TestCookieSessionsNeedTheCSRFToken(test *testing.T) {
	current := newHarness(test, nil)
	cookie, csrf := sessionCookieFor(current, RoleOperator, "")
	target := "/api/v1/campaigns/" + campaignIdentifier.String() + "/pause"
	send := func(header string, csrfCookie string) response {
		request := newRequest(http.MethodPost, target, map[string]any{"reason": "investigating"}, "")
		request.AddCookie(cookie)
		if csrfCookie != "" {
			request.AddCookie(&http.Cookie{Name: CSRFCookie, Value: csrfCookie})
		}
		if header != "" {
			request.Header.Set(CSRFHeader, header)
		}
		return current.do(request)
	}
	other := randomSecret()
	for name, result := range map[string]response{
		"no header":                 send("", csrf),
		"no cookie":                 send(csrf, ""),
		"header and cookie differ":  send(csrf, other),
		"another session's token":   send(other, other),
		"a token of the wrong size": send("x", "x"),
	} {
		if result.Code != http.StatusForbidden || errorCode(result) != "csrf_failed" {
			test.Errorf("%s: %d %s", name, result.Code, result.Body.String())
		}
	}
	if current.backend.last("PauseCampaign") != nil {
		test.Fatal("a request without its CSRF token reached the backend")
	}
	if accepted := send(csrf, csrf); accepted.Code != http.StatusOK {
		test.Fatalf("a matching token answered %d %s", accepted.Code, accepted.Body.String())
	}
	if arguments := current.backend.last("PauseCampaign"); arguments[1] != "user-operator" || arguments[2] != "investigating" {
		test.Fatalf("PauseCampaign %v", arguments)
	}
	read := newRequest(http.MethodGet, "/api/v1/me", nil, "")
	read.AddCookie(cookie)
	me := current.do(read)
	if me.Code != http.StatusOK || field(me, "authentication") != "oidc" || field(me, "name") != "Ada Operator" || field(me, "expires_at") == nil {
		test.Fatalf("a read with a session: %d %s", me.Code, me.Body.String())
	}
	if bearer := current.call(http.MethodPost, target, map[string]any{}, operatorToken); bearer.Code != http.StatusOK {
		test.Fatalf("an API token needs no CSRF token, answered %d", bearer.Code)
	}
}

func TestSessionsEndWhenIdleOrExpired(test *testing.T) {
	current := newHarness(test, nil)
	cookie, _ := sessionCookieFor(current, RoleViewer, "")
	read := func() response {
		request := newRequest(http.MethodGet, "/api/v1/me", nil, "")
		request.AddCookie(cookie)
		return current.do(request)
	}
	current.now = current.now.Add(50 * time.Minute)
	if result := read(); result.Code != http.StatusOK {
		test.Fatalf("an active session answered %d", result.Code)
	}
	current.now = current.now.Add(50 * time.Minute)
	if result := read(); result.Code != http.StatusOK {
		test.Fatalf("a session used 50 minutes ago answered %d; using it must have kept it alive", result.Code)
	}
	current.now = current.now.Add(61 * time.Minute)
	if result := read(); result.Code != http.StatusUnauthorized {
		test.Fatalf("an idle session answered %d", result.Code)
	}
	if len(current.sessions.sessions) != 0 {
		test.Fatal("an ended session was kept")
	}
	cookie, _ = sessionCookieFor(current, RoleViewer, "")
	for range 14 {
		current.now = current.now.Add(55 * time.Minute)
		read()
	}
	if result := read(); result.Code != http.StatusUnauthorized {
		test.Fatalf("a session past its lifetime answered %d", result.Code)
	}
	garbage := newRequest(http.MethodGet, "/api/v1/me", nil, "")
	garbage.AddCookie(&http.Cookie{Name: SessionCookie, Value: "not-a-session"})
	if result := current.do(garbage); result.Code != http.StatusUnauthorized {
		test.Fatalf("a malformed session cookie answered %d", result.Code)
	}
}

func TestLogoutEndsTheSession(test *testing.T) {
	current := newHarness(test, nil)
	cookie, csrf := sessionCookieFor(current, RoleViewer, "")
	request := newRequest(http.MethodPost, "/api/v1/auth/logout", nil, "")
	request.AddCookie(cookie)
	request.AddCookie(&http.Cookie{Name: CSRFCookie, Value: csrf})
	request.Header.Set(CSRFHeader, csrf)
	result := current.do(request)
	if result.Code != http.StatusNoContent || len(current.sessions.sessions) != 0 {
		test.Fatalf("%d, %d sessions left", result.Code, len(current.sessions.sessions))
	}
	cleared := 0
	for _, set := range result.Result().Cookies() {
		if (set.Name == SessionCookie || set.Name == CSRFCookie) && set.MaxAge < 0 {
			cleared++
		}
	}
	if cleared != 2 {
		test.Fatalf("cookies %v", result.Result().Cookies())
	}
}

func TestDevelopmentLoginIsForLoopbackCallersOnly(test *testing.T) {
	current := newHarness(test, func(options *Options) { options.Development = true })
	request := newRequest(http.MethodGet, LoginPath+"?return_to=/campaigns", nil, "")
	request.RemoteAddr = "127.0.0.1:50000"
	result := current.do(request)
	if result.Code != http.StatusSeeOther || result.Header().Get("Location") != "/campaigns" {
		test.Fatalf("%d %s", result.Code, result.Header())
	}
	var session, csrf *http.Cookie
	for _, set := range result.Result().Cookies() {
		switch set.Name {
		case SessionCookie:
			session = set
		case CSRFCookie:
			csrf = set
		}
	}
	if session == nil || csrf == nil || !session.HttpOnly || csrf.HttpOnly || session.Secure || session.SameSite != http.SameSiteLaxMode {
		test.Fatalf("cookies %v", result.Result().Cookies())
	}
	read := newRequest(http.MethodGet, "/api/v1/me", nil, "")
	read.AddCookie(session)
	if me := current.do(read); field(me, "role") != "admin" || field(me, "authentication") != "dev" || field(me, "subject") != "dev" {
		test.Fatalf("%s", me.Body.String())
	}
	remote := newRequest(http.MethodGet, LoginPath, nil, "")
	remote.RemoteAddr = "192.0.2.10:50000"
	if refused := current.do(remote); refused.Code != http.StatusForbidden {
		test.Fatalf("a remote development login answered %d", refused.Code)
	}
	for _, target := range []string{"https://evil.example/", "//evil.example/", `/\evil.example`, "campaigns"} {
		redirect := newRequest(http.MethodGet, LoginPath+"?return_to="+url.QueryEscape(target), nil, "")
		redirect.RemoteAddr = "127.0.0.1:50000"
		if refused := current.do(redirect); refused.Code != http.StatusBadRequest {
			test.Errorf("return_to %q answered %d", target, refused.Code)
		}
	}
}

func TestSecureCookiesOutsideDevelopment(test *testing.T) {
	current := newHarness(test, nil)
	cookie := current.server.cookie(SessionCookie, "value", time.Hour, true, "/")
	if !cookie.Secure || !cookie.HttpOnly || cookie.SameSite != http.SameSiteLaxMode || cookie.MaxAge != 3600 {
		test.Fatalf("%+v", cookie)
	}
}
