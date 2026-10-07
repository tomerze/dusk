package api

import (
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"errors"
	"net/http"
	"strings"
	"time"

	"dusk/services/twilight/internal/tokens"
)

const (
	SessionCookie = "twilight_session"
	CSRFCookie    = "twilight_csrf"
	CSRFHeader    = "X-CSRF-Token"
	LoginCookie   = "twilight_login"
	LoginPath     = "/api/v1/auth/login"
	CallbackPath  = "/api/v1/auth/callback"
)

func randomSecret() string {
	material := make([]byte, 32)
	_, _ = rand.Read(material)
	return base64.RawURLEncoding.EncodeToString(material)
}

func digest(secret string) []byte {
	sum := sha256.Sum256([]byte(secret))
	return sum[:]
}

func wellFormedSecret(secret string) bool {
	decoded, failure := base64.RawURLEncoding.DecodeString(secret)
	return failure == nil && len(decoded) == 32
}

func (server *Server) unauthenticated(call *exchange, reason, message string) *Problem {
	authenticationFailures.WithLabelValues(reason).Inc()
	server.Logger.Warn("api request not authenticated", "reason", reason, "remote_address", call.request.RemoteAddr, "path", call.request.URL.Path, "request_id", call.requestID)
	problem := newProblem(http.StatusUnauthorized, "unauthenticated", message)
	if server.OIDC != nil || server.Development {
		problem.with("login", LoginPath)
	}
	return problem
}

func (server *Server) authenticate(call *exchange) (*Principal, error) {
	request := call.request
	if header := request.Header.Get("Authorization"); header != "" {
		scheme, secret, _ := strings.Cut(header, " ")
		secret = strings.TrimSpace(secret)
		if !strings.EqualFold(scheme, "Bearer") || secret == "" {
			return nil, server.unauthenticated(call, "malformed_authorization", "the Authorization header must be Bearer followed by an API token")
		}
		token, failure := server.Tokens.Authenticate(request.Context(), secret)
		if errors.Is(failure, tokens.ErrInvalid) {
			return nil, server.unauthenticated(call, "token", "the API token is unknown or revoked")
		}
		if failure != nil {
			return nil, failure
		}
		role, _ := ParseRole(token.Role)
		name := token.Name
		return &Principal{Subject: "token:" + token.ID.String(), Name: &name, Role: role.String(), Authentication: AuthenticationToken, role: role}, nil
	}
	if cookie, failure := request.Cookie(SessionCookie); failure == nil && cookie.Value != "" {
		return server.sessionPrincipal(call, cookie.Value)
	}
	if request.TLS != nil && len(request.TLS.VerifiedChains) > 0 && len(request.TLS.VerifiedChains[0]) > 0 {
		name, failure := CertificatePrincipal(request.TLS.VerifiedChains[0][0])
		if failure != nil {
			authenticationFailures.WithLabelValues("certificate").Inc()
			server.Logger.Warn("api client certificate refused", "error", failure, "remote_address", request.RemoteAddr, "request_id", call.requestID)
			return nil, newProblem(http.StatusForbidden, "forbidden", failure.Error())
		}
		role := server.Principals.RoleOf(name)
		if role == RolePublic {
			authenticationFailures.WithLabelValues("principal").Inc()
			server.Logger.Warn("api principal has no role", "principal", name, "remote_address", request.RemoteAddr, "request_id", call.requestID)
			return nil, newProblem(http.StatusForbidden, "forbidden", "the principal "+name+" has no role in twilight")
		}
		return &Principal{Subject: name, Role: role.String(), Authentication: AuthenticationMTLS, role: role}, nil
	}
	return nil, server.unauthenticated(call, "missing", "log in, or send an API token as Authorization: Bearer")
}

func (server *Server) sessionPrincipal(call *exchange, secret string) (*Principal, error) {
	if !wellFormedSecret(secret) {
		return nil, server.unauthenticated(call, "session", "the session has ended; log in again")
	}
	operation := call.request.Context()
	key := digest(secret)
	session, failure := server.Sessions.Session(operation, key)
	if errors.Is(failure, ErrSessionNotFound) {
		return nil, server.unauthenticated(call, "session", "the session has ended; log in again")
	}
	if failure != nil {
		return nil, failure
	}
	now := server.Now()
	if !now.Before(session.ExpiresAt) || now.Sub(session.LastSeenAt) >= server.SessionIdle {
		if failure := server.Sessions.DeleteSession(operation, key); failure != nil {
			server.Logger.Warn("an ended session was not deleted", "principal", session.Subject, "error", failure)
		}
		server.Logger.Info("login session ended", "principal", session.Subject, "reason", "expired")
		return nil, server.unauthenticated(call, "session", "the session has ended; log in again")
	}
	if now.Sub(session.LastSeenAt) >= time.Minute {
		if failure := server.Sessions.TouchSession(operation, key, now); failure != nil {
			server.Logger.Warn("the session's last use was not recorded", "principal", session.Subject, "error", failure)
		}
	}
	expires := session.ExpiresAt
	if idle := now.Add(server.SessionIdle); idle.Before(expires) {
		expires = idle
	}
	return &Principal{Subject: session.Subject, Name: session.Name, Role: session.Role.String(), Authentication: session.Authentication, ExpiresAt: &expires, role: session.Role, session: &session}, nil
}

func (server *Server) checkCSRF(call *exchange, session *Session) error {
	header := call.request.Header.Get(CSRFHeader)
	cookie, failure := call.request.Cookie(CSRFCookie)
	if header == "" || failure != nil || subtle.ConstantTimeCompare([]byte(header), []byte(cookie.Value)) != 1 || subtle.ConstantTimeCompare(digest(header), session.CSRFDigest) != 1 {
		authenticationFailures.WithLabelValues("csrf").Inc()
		server.Logger.Warn("api request refused for its CSRF token", "principal", session.Subject, "path", call.request.URL.Path, "request_id", call.requestID)
		return newProblem(http.StatusForbidden, "csrf_failed", "send the "+CSRFCookie+" cookie's value in the "+CSRFHeader+" header")
	}
	return nil
}

func (server *Server) cookie(name, value string, maximumAge time.Duration, httpOnly bool, path string) *http.Cookie {
	cookie := &http.Cookie{Name: name, Value: value, Path: path, HttpOnly: httpOnly, Secure: !server.Development, SameSite: http.SameSiteLaxMode, MaxAge: int(maximumAge.Seconds())}
	if maximumAge < 0 {
		cookie.MaxAge = -1
	}
	return cookie
}

func (server *Server) startSession(call *exchange, subject string, name *string, role Role, authentication, returnTo string) error {
	secret, csrf := randomSecret(), randomSecret()
	now := server.Now()
	session := Session{Digest: digest(secret), CSRFDigest: digest(csrf), Subject: subject, Name: name, Role: role, Authentication: authentication,
		CreatedAt: now, LastSeenAt: now, ExpiresAt: now.Add(server.SessionLifetime)}
	if failure := server.Sessions.CreateSession(call.request.Context(), session, now.Add(-server.SessionIdle)); failure != nil {
		return failure
	}
	http.SetCookie(call.writer, server.cookie(SessionCookie, secret, server.SessionLifetime, true, "/"))
	http.SetCookie(call.writer, server.cookie(CSRFCookie, csrf, server.SessionLifetime, false, "/"))
	server.Logger.Info("login session started", "principal", subject, "role", role.String(), "authentication", authentication, "remote_address", call.request.RemoteAddr)
	http.Redirect(call.writer, call.request, returnTo, http.StatusSeeOther)
	return nil
}

func (server *Server) logout(call *exchange) error {
	if session := call.principal.session; session != nil {
		if failure := server.Sessions.DeleteSession(call.request.Context(), session.Digest); failure != nil {
			return failure
		}
		server.Logger.Info("login session ended", "principal", session.Subject, "reason", "logout")
	}
	http.SetCookie(call.writer, server.cookie(SessionCookie, "", -1, true, "/"))
	http.SetCookie(call.writer, server.cookie(CSRFCookie, "", -1, false, "/"))
	call.writer.WriteHeader(http.StatusNoContent)
	return nil
}
