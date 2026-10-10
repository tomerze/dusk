package alerts

import (
	"bytes"
	"context"
	"crypto/tls"
	"errors"
	"fmt"
	"html/template"
	"log/slog"
	"mime"
	"mime/multipart"
	"mime/quotedprintable"
	"net"
	"net/smtp"
	"net/textproto"
	"slices"
	"strconv"
	"strings"
	"time"

	"dusk/services/twilight/internal/config"
)

const (
	emailDialTimeout    = 10 * time.Second
	emailSessionTimeout = 30 * time.Second
)

type emailSender struct {
	settings config.EmailReceiver
	logger   *slog.Logger
}

type loginAuth struct {
	username string
	password string
}

func (auth loginAuth) Start(server *smtp.ServerInfo) (string, []byte, error) {
	if !server.TLS {
		return "", nil, errors.New("unencrypted connection")
	}
	return "LOGIN", nil, nil
}

func (auth loginAuth) Next(challenge []byte, more bool) ([]byte, error) {
	if !more {
		return nil, nil
	}
	switch strings.ToLower(strings.TrimSpace(string(challenge))) {
	case "username:", "user name", "username":
		return []byte(auth.username), nil
	case "password:", "password":
		return []byte(auth.password), nil
	}
	return nil, errors.New("the server sent an unexpected LOGIN challenge")
}

var emailTemplate = template.Must(template.New("email").Parse(`<!doctype html>
<html><body style="font-family: -apple-system, 'Segoe UI', Helvetica, Arial, sans-serif; color: #1f2328; font-size: 14px;">
<h2 style="margin: 0 0 8px 0; font-size: 18px;">{{.Title}}</h2>
<p style="margin: 0 0 4px 0;">{{.Summary}}</p>
<p style="margin: 0 0 16px 0; color: #59636e;">{{.Change}}</p>
{{if .Facts}}<table style="border-collapse: collapse; margin-bottom: 16px;">{{range .Facts}}
<tr><th style="text-align: left; padding: 2px 16px 2px 0; vertical-align: top;">{{.Label}}</th><td style="padding: 2px 0; font-family: ui-monospace, Menlo, Consolas, monospace;">{{.Value}}</td></tr>{{end}}
</table>{{end}}
{{if .Evidence}}<h3 style="margin: 0 0 4px 0; font-size: 14px;">Evidence</h3>
<table style="border-collapse: collapse; margin-bottom: 16px;">{{range .Evidence}}
<tr><th style="text-align: left; padding: 2px 16px 2px 0; vertical-align: top; font-family: ui-monospace, Menlo, Consolas, monospace; font-weight: normal;">{{.Label}}</th><td style="padding: 2px 0; font-family: ui-monospace, Menlo, Consolas, monospace;">{{.Value}}</td></tr>{{end}}
</table>{{end}}
<p>{{range $index, $link := .Links}}{{if $index}} &middot; {{end}}<a href="{{$link.Value}}">{{$link.Label}}</a>{{end}}</p>
</body></html>
`))

type emailRow struct {
	Label string
	Value string
}

func emailRows(facts []fact) []emailRow {
	rows := make([]emailRow, 0, len(facts))
	for _, entry := range facts {
		rows = append(rows, emailRow{Label: entry.label, Value: entry.value})
	}
	return rows
}

func emailDomain(address string) string {
	if _, domain, found := strings.Cut(address, "@"); found && domain != "" {
		return domain
	}
	return "twilight.invalid"
}

func emailThread(message notification, domain string) string {
	return fmt.Sprintf("<alert-%d-%d.%s@%s>", message.alert.ID, message.alert.Time.UnixMicro(), message.receiver, domain)
}

func writePart(writer *multipart.Writer, contentType, content string) error {
	part, failure := writer.CreatePart(textproto.MIMEHeader{
		"Content-Type":              {contentType + "; charset=utf-8"},
		"Content-Transfer-Encoding": {"quoted-printable"},
	})
	if failure != nil {
		return failure
	}
	encoder := quotedprintable.NewWriter(part)
	if _, failure := encoder.Write([]byte(content)); failure != nil {
		return failure
	}
	return encoder.Close()
}

func emailPayload(settings config.EmailReceiver, message notification, now time.Time) ([]byte, error) {
	var html bytes.Buffer
	view := map[string]any{"Title": message.title(), "Summary": message.summary(), "Change": message.change(), "Links": emailRows(message.links())}
	if message.transition == TransitionOpened || message.transition == TransitionReEscalated {
		view["Facts"], view["Evidence"] = emailRows(message.facts()), emailRows(message.evidence())
	}
	if failure := emailTemplate.Execute(&html, view); failure != nil {
		return nil, failure
	}
	domain := emailDomain(settings.From)
	thread := emailThread(message, domain)
	identifier := thread
	if message.transition != TransitionOpened {
		identifier = "<" + message.key + "@" + domain + ">"
	}
	var body bytes.Buffer
	writer := multipart.NewWriter(&body)
	var headers bytes.Buffer
	for _, header := range [][2]string{
		{"From", settings.From},
		{"To", strings.Join(settings.To, ", ")},
		{"Subject", mime.QEncoding.Encode("utf-8", message.title())},
		{"Date", now.Format(time.RFC1123Z)},
		{"Message-ID", identifier},
		{"Auto-Submitted", "auto-generated"},
		{"MIME-Version", "1.0"},
		{"Content-Type", "multipart/alternative; boundary=" + writer.Boundary()},
	} {
		headers.WriteString(header[0] + ": " + header[1] + "\r\n")
	}
	if identifier != thread {
		headers.WriteString("In-Reply-To: " + thread + "\r\nReferences: " + thread + "\r\n")
	}
	headers.WriteString("\r\n")
	if failure := writePart(writer, "text/plain", message.plainText()); failure != nil {
		return nil, failure
	}
	if failure := writePart(writer, "text/html", html.String()); failure != nil {
		return nil, failure
	}
	if failure := writer.Close(); failure != nil {
		return nil, failure
	}
	return append(headers.Bytes(), body.Bytes()...), nil
}

func smtpFailure(stage string, failure error) error {
	var answered *textproto.Error
	if errors.As(failure, &answered) {
		return deliveryError{message: fmt.Sprintf("SMTP %s: %d %s", stage, answered.Code, printable(answered.Msg)), permanent: answered.Code >= 500}
	}
	return deliveryError{message: "SMTP " + stage + ": " + printable(failure.Error())}
}

func (sender emailSender) send(operation context.Context, message notification) error {
	settings := sender.settings
	var password string
	if settings.PasswordFile != "" {
		secret, failure := readSecret(settings.PasswordFile)
		if failure != nil {
			return deliveryError{message: failure.Error()}
		}
		password = secret
	}
	raw, failure := emailPayload(settings, message, time.Now())
	if failure != nil {
		return deliveryError{message: "the email could not be built: " + failure.Error(), permanent: true}
	}
	address := net.JoinHostPort(settings.Host, strconv.Itoa(settings.PortNumber()))
	secured := &tls.Config{ServerName: settings.Host, MinVersion: tls.VersionTLS12}
	dialer := &net.Dialer{Timeout: emailDialTimeout}
	var connection net.Conn
	if settings.SecurityMode() == config.EmailTLS {
		connection, failure = (&tls.Dialer{NetDialer: dialer, Config: secured}).DialContext(operation, "tcp", address)
	} else {
		connection, failure = dialer.DialContext(operation, "tcp", address)
	}
	if failure != nil {
		return smtpFailure("connect", failure)
	}
	deadline := time.Now().Add(emailSessionTimeout)
	if limit, set := operation.Deadline(); set && limit.Before(deadline) {
		deadline = limit
	}
	if failure := connection.SetDeadline(deadline); failure != nil {
		connection.Close()
		return smtpFailure("connect", failure)
	}
	client, failure := smtp.NewClient(connection, settings.Host)
	if failure != nil {
		connection.Close()
		return smtpFailure("greeting", failure)
	}
	defer client.Close()
	if settings.SecurityMode() == config.EmailStartTLS {
		if supported, _ := client.Extension("STARTTLS"); !supported {
			return deliveryError{message: "SMTP starttls: the server does not offer STARTTLS, and email.security is starttls"}
		}
		if failure := client.StartTLS(secured); failure != nil {
			return smtpFailure("starttls", failure)
		}
	}
	if settings.Username != "" {
		_, offered := client.Extension("AUTH")
		mechanisms := strings.Fields(strings.ToUpper(offered))
		var auth smtp.Auth
		switch {
		case slices.Contains(mechanisms, "PLAIN"):
			auth = smtp.PlainAuth("", settings.Username, password, settings.Host)
		case slices.Contains(mechanisms, "LOGIN"):
			auth = loginAuth{username: settings.Username, password: password}
		default:
			return deliveryError{message: "SMTP auth: the server offers neither PLAIN nor LOGIN"}
		}
		if failure := client.Auth(auth); failure != nil {
			return smtpFailure("auth", failure)
		}
	}
	if failure := client.Mail(settings.From); failure != nil {
		return smtpFailure("mail from", failure)
	}
	for _, recipient := range settings.To {
		if failure := client.Rcpt(recipient); failure != nil {
			return smtpFailure("rcpt to", failure)
		}
	}
	data, failure := client.Data()
	if failure != nil {
		return smtpFailure("data", failure)
	}
	if _, failure := data.Write(raw); failure != nil {
		return smtpFailure("data", failure)
	}
	if failure := data.Close(); failure != nil {
		return smtpFailure("data", failure)
	}
	if failure := client.Quit(); failure != nil {
		sender.logger.Warn("the SMTP server accepted an alert notification, then QUIT failed", "alert_id", message.alert.ID, "receiver", message.receiver, "error", printable(failure.Error()))
	}
	return nil
}
