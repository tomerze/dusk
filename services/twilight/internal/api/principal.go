package api

import (
	"crypto/x509"
	"errors"
	"path"
	"sort"
	"strings"
	"time"
)

type Role int

const (
	RolePublic Role = iota
	RoleViewer
	RoleOperator
	RoleAdmin
)

var roleNames = map[Role]string{RolePublic: "public", RoleViewer: "viewer", RoleOperator: "operator", RoleAdmin: "admin"}

func (role Role) String() string {
	return roleNames[role]
}

func ParseRole(text string) (Role, bool) {
	for role, name := range roleNames {
		if name == text && role != RolePublic {
			return role, true
		}
	}
	return RolePublic, false
}

const (
	AuthenticationOIDC        = "oidc"
	AuthenticationToken       = "token"
	AuthenticationMTLS        = "mtls"
	AuthenticationDevelopment = "dev"
)

type Principal struct {
	Subject        string     `json:"subject"`
	Name           *string    `json:"name"`
	Role           string     `json:"role"`
	Authentication string     `json:"authentication"`
	ExpiresAt      *time.Time `json:"expires_at"`
	role           Role
	session        *Session
}

type PrincipalRoles map[string]Role

func ParsePrincipalRoles(patterns map[string]string) (PrincipalRoles, error) {
	roles := PrincipalRoles{}
	for pattern, name := range patterns {
		role, known := ParseRole(name)
		if !known {
			return nil, errors.New("principals maps " + pattern + " to an unknown role " + name)
		}
		if _, failure := path.Match(pattern, ""); failure != nil {
			return nil, errors.New("principals holds an invalid pattern " + pattern)
		}
		roles[pattern] = role
	}
	return roles, nil
}

func (roles PrincipalRoles) RoleOf(name string) Role {
	granted := RolePublic
	for pattern, role := range roles {
		if matched, _ := path.Match(pattern, name); matched && role > granted {
			granted = role
		}
	}
	return granted
}

const (
	principalPrefix    = "urn:dusk:principal:"
	devicePrefix       = "urn:dusk:device:"
	installationPrefix = "urn:dusk:installation:"
)

var (
	ErrNoPrincipal       = errors.New("the client certificate names no urn:dusk:principal")
	ErrSeveralPrincipals = errors.New("the client certificate names more than one urn:dusk:principal")
	ErrNodeCertificate   = errors.New("the client certificate is a node's")
)

func CertificatePrincipal(certificate *x509.Certificate) (string, error) {
	var names []string
	for _, uri := range certificate.URIs {
		text := uri.String()
		switch {
		case strings.HasPrefix(text, devicePrefix), strings.HasPrefix(text, installationPrefix):
			return "", ErrNodeCertificate
		case strings.HasPrefix(text, principalPrefix):
			names = append(names, strings.TrimPrefix(text, principalPrefix))
		}
	}
	sort.Strings(names)
	switch {
	case len(names) == 0 || names[0] == "":
		return "", ErrNoPrincipal
	case len(names) > 1:
		return "", ErrSeveralPrincipals
	}
	return names[0], nil
}
