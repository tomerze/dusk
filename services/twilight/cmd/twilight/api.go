package main

import (
	"fmt"
	"log/slog"
	"net"
	"net/http"
	"os"
	"strings"
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

func apiListener(settings config.Config, logger *slog.Logger) (net.Listener, error) {
	listener, failure := net.Listen("tcp", settings.Listen)
	if failure != nil {
		return nil, fmt.Errorf("listen on %s: %w", settings.Listen, failure)
	}
	return listener, nil
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
