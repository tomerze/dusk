package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"syscall"
)

const usage = `twilight-scale loads the Dusk stack with campaigns.

Usage:
  twilight-scale load --token-file FILE [flags]

load creates and starts campaigns through twilight's API, each over its own
range of simulated nodes, and waits for them to finish.

twilight-scale load --help lists the flags.
`

type usageError struct{ message string }

func (failure usageError) Error() string { return failure.message }

func main() {
	signals, stop := signal.NotifyContext(context.Background(), syscall.SIGTERM, os.Interrupt)
	defer stop()
	os.Exit(run(signals, os.Args[1:], os.Stdout, os.Stderr))
}

func run(operation context.Context, arguments []string, output, diagnostics io.Writer) int {
	if len(arguments) == 0 {
		fmt.Fprint(diagnostics, usage)
		return 2
	}
	var verdict int
	var failure error
	switch arguments[0] {
	case "load":
		verdict, failure = load(operation, arguments[1:], output, diagnostics)
	case "help", "-h", "--help":
		fmt.Fprint(output, usage)
		return 0
	default:
		failure = usageError{fmt.Sprintf("unknown command %q", arguments[0])}
	}
	var wrongUsage usageError
	switch {
	case failure == nil:
		return verdict
	case errors.Is(failure, flag.ErrHelp):
		return 0
	case errors.As(failure, &wrongUsage):
		fmt.Fprintf(diagnostics, "twilight-scale: %v\n\n", failure)
		fmt.Fprint(diagnostics, usage)
		return 2
	default:
		fmt.Fprintf(diagnostics, "twilight-scale: %v\n", failure)
		return 2
	}
}

func parse(name string, arguments []string, diagnostics io.Writer, define func(*flag.FlagSet)) error {
	flags := flag.NewFlagSet("twilight-scale "+name, flag.ContinueOnError)
	flags.SetOutput(diagnostics)
	define(flags)
	if failure := flags.Parse(arguments); failure != nil {
		if errors.Is(failure, flag.ErrHelp) {
			return failure
		}
		return usageError{failure.Error()}
	}
	if flags.NArg() > 0 {
		return usageError{fmt.Sprintf("%s takes no arguments besides its flags", name)}
	}
	return nil
}
