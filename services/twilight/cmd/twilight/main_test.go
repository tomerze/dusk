package main

import (
	"bytes"
	"context"
	"path/filepath"
	"strings"
	"testing"
)

func invoke(arguments ...string) (int, string, string) {
	var output, diagnostics bytes.Buffer
	code := run(context.Background(), arguments, nil, &output, &diagnostics)
	return code, output.String(), diagnostics.String()
}

func TestVersionAndHelp(test *testing.T) {
	code, output, _ := invoke("version")
	if code != 0 || !strings.HasPrefix(output, "twilight dev (") {
		test.Fatalf("version: %d %q", code, output)
	}
	code, output, _ = invoke("help")
	if code != 0 || !strings.Contains(output, "twilight token create --name NAME") {
		test.Fatalf("help: %d %q", code, output)
	}
}

func TestUsageMistakesExitWithTwo(test *testing.T) {
	for _, arguments := range [][]string{
		{},
		{"launch"},
		{"token"},
		{"token", "rotate"},
		{"token", "create", "--role", "admin"},
		{"token", "create", "--name", "ci", "--role", "root"},
		{"token", "revoke"},
		{"token", "revoke", "not-a-uuid"},
		{"migrate", "extra"},
		{"serve", "--no-such-flag"},
		{"serve", "now"},
		{"token", "revoke", "0192f3a4-0000-7000-8000-000000000000", "--config"},
		{"token", "revoke", "0192f3a4-0000-7000-8000-000000000000", "0192f3a4-0000-7000-8000-000000000001"},
	} {
		code, _, diagnostics := invoke(arguments...)
		if code != 2 || !strings.Contains(diagnostics, "Usage:") {
			test.Errorf("%v: exit %d, %q", arguments, code, diagnostics)
		}
	}
}

func TestAnExplicitMissingConfigurationFails(test *testing.T) {
	missing := filepath.Join(test.TempDir(), "twilight.yaml")
	for _, command := range []string{"serve", "migrate"} {
		code, _, diagnostics := invoke(command, "--config", missing)
		if code != 1 || !strings.Contains(diagnostics, missing) {
			test.Errorf("%s: exit %d, %q", command, code, diagnostics)
		}
	}
}
