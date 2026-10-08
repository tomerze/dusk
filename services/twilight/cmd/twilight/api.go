package main

import (
	"crypto/tls"
	"crypto/x509"
	"errors"
	"fmt"
	"log/slog"
	"net"
	"net/http"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"

	"dusk/services/twilight/internal/api"
	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/tokens"
	"dusk/services/twilight/internal/webui"
	"dusk/services/twilight/web"
)

func apiOptions(settings config.Config, pool *pgxpool.Pool, backend api.Backend, logger *slog.Logger) (api.Options, error) {
	principals, failure := api.ParsePrincipalRoles(settings.Principals)
	if failure != nil {
		return api.Options{}, failure
	}
	assets, available := web.Assets()
	pages, failure := webui.Handler(assets, available)
	if failure != nil {
		return api.Options{}, failure
	}
	if !available {
		logger.Warn("this binary was built without the UI; its pages answer 503 UI not built")
	}
	options := api.Options{
		Backend:         backend,
		Tokens:          tokens.NewStore(pool),
		Sessions:        api.NewPostgresSessions(pool),
		Principals:      principals,
		SessionLifetime: time.Duration(settings.Sessions.LifetimeSeconds) * time.Second,
		SessionIdle:     time.Duration(settings.Sessions.IdleSeconds) * time.Second,
		MaxStreams:      settings.MaxStreams,
		MaxRequests:     settings.MaxRequests,
		RequestTimeout:  time.Duration(settings.RequestTimeoutSeconds) * time.Second,
		UI:              pages,
		Logger:          logger,
	}
	if settings.OIDC.Issuer != "" {
		secret := ""
		if settings.OIDC.ClientSecretFile != "" {
			content, failure := os.ReadFile(settings.OIDC.ClientSecretFile)
			if failure != nil {
				return api.Options{}, fmt.Errorf("read oidc.client_secret_file: %w", failure)
			}
			secret = strings.TrimSpace(string(content))
		}
		roles := map[string]api.Role{}
		for value, name := range settings.OIDC.RoleMap {
			role, _ := api.ParseRole(name)
			roles[value] = role
		}
		options.OIDC = &api.OIDCSettings{
			Issuer: settings.OIDC.Issuer, ClientID: settings.OIDC.ClientID, ClientSecret: secret, RedirectURL: settings.OIDC.RedirectURL,
			Scopes: settings.OIDC.Scopes, RoleClaim: settings.OIDC.RoleClaim, RoleMap: roles, HTTPClient: &http.Client{Timeout: 10 * time.Second},
		}
	}
	return options, nil
}

type certificateFiles struct {
	certificate, key string
	logger           *slog.Logger
	mutex            sync.Mutex
	loaded           *tls.Certificate
	modified         time.Time
	checked          time.Time
}

func (files *certificateFiles) current(*tls.ClientHelloInfo) (*tls.Certificate, error) {
	files.mutex.Lock()
	defer files.mutex.Unlock()
	if files.loaded != nil && time.Since(files.checked) < 30*time.Second {
		return files.loaded, nil
	}
	files.checked = time.Now()
	var latest time.Time
	for _, path := range []string{files.certificate, files.key} {
		information, failure := os.Stat(path)
		if failure != nil {
			if files.loaded != nil {
				files.logger.Warn("the API's certificate could not be checked; serving the one loaded", "path", path, "error", failure)
				return files.loaded, nil
			}
			return nil, failure
		}
		if information.ModTime().After(latest) {
			latest = information.ModTime()
		}
	}
	if files.loaded != nil && !latest.After(files.modified) {
		return files.loaded, nil
	}
	pair, failure := tls.LoadX509KeyPair(files.certificate, files.key)
	if failure != nil {
		if files.loaded != nil {
			files.logger.Warn("the API's renewed certificate could not be loaded; serving the previous one", "error", failure)
			return files.loaded, nil
		}
		return nil, failure
	}
	files.loaded, files.modified = &pair, latest
	files.logger.Info("the API's certificate was loaded", "certificate", files.certificate)
	return files.loaded, nil
}

func apiListener(settings config.Config, logger *slog.Logger) (net.Listener, error) {
	listener, failure := net.Listen("tcp", settings.Listen)
	if failure != nil {
		return nil, fmt.Errorf("listen on %s: %w", settings.Listen, failure)
	}
	if settings.TLS.Certificate == "" {
		return listener, nil
	}
	files := &certificateFiles{certificate: settings.TLS.Certificate, key: settings.TLS.Key, logger: logger}
	if _, failure := files.current(nil); failure != nil {
		listener.Close()
		return nil, fmt.Errorf("load tls.certificate and tls.key: %w", failure)
	}
	configuration := &tls.Config{MinVersion: tls.VersionTLS12, GetCertificate: files.current, NextProtos: []string{"h2", "http/1.1"}}
	if settings.TLS.ClientCA != "" {
		content, failure := os.ReadFile(settings.TLS.ClientCA)
		if failure != nil {
			listener.Close()
			return nil, fmt.Errorf("read tls.client_ca: %w", failure)
		}
		pool := x509.NewCertPool()
		if !pool.AppendCertsFromPEM(content) {
			listener.Close()
			return nil, errors.New("tls.client_ca holds no PEM certificate")
		}
		configuration.ClientCAs = pool
		configuration.ClientAuth = tls.VerifyClientCertIfGiven
	}
	return tls.NewListener(listener, configuration), nil
}

func apiServer(handler http.Handler, logger *slog.Logger) *http.Server {
	return &http.Server{
		Handler:           handler,
		ReadHeaderTimeout: 10 * time.Second,
		ReadTimeout:       60 * time.Second,
		WriteTimeout:      60 * time.Second,
		IdleTimeout:       2 * time.Minute,
		MaxHeaderBytes:    64 << 10,
		ErrorLog:          slog.NewLogLogger(logger.Handler(), slog.LevelWarn),
	}
}
