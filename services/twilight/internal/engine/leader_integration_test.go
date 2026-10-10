//go:build integration

package engine_test

import (
	"context"
	"fmt"
	"testing"
	"time"

	"dusk/services/twilight/internal/engine"
	"dusk/services/twilight/internal/testsupport"
)

func TestMain(suite *testing.M) {
	testsupport.Main(suite)
}

func TestLeadershipFailsOver(test *testing.T) {
	pool, url := testsupport.Database(test)
	elected := make(chan string, 8)
	start := func(name string) (*engine.Leadership, context.CancelFunc) {
		leadership := engine.NewLeadership(url, name, testsupport.Logger(), func(operation context.Context, term int64) {
			elected <- fmt.Sprintf("%s:%d", name, term)
			<-operation.Done()
		})
		operation, cancel := context.WithCancel(context.Background())
		go leadership.Run(operation)
		return leadership, cancel
	}
	firstLeadership, stopFirst := start("first")
	defer stopFirst()
	var winner string
	select {
	case winner = <-elected:
	case <-time.After(30 * time.Second):
		test.Fatal("nobody was elected")
	}
	if winner != "first:1" || !firstLeadership.Leading() || firstLeadership.Term() != 1 {
		test.Fatalf("first election %s", winner)
	}
	secondLeadership, stopSecond := start("second")
	defer stopSecond()
	time.Sleep(3 * time.Second)
	if secondLeadership.Leading() {
		test.Fatal("two leaders at once")
	}
	if _, failure := pool.Exec(context.Background(), `select pg_terminate_backend(pid) from pg_stat_activity where application_name = 'twilight-leader-first'`); failure != nil {
		test.Fatal(failure)
	}
	select {
	case winner = <-elected:
	case <-time.After(30 * time.Second):
		test.Fatal("no failover")
	}
	if winner != "second:2" {
		test.Fatalf("failover elected %s", winner)
	}
	deadline := time.Now().Add(10 * time.Second)
	for firstLeadership.Leading() && time.Now().Before(deadline) {
		time.Sleep(100 * time.Millisecond)
	}
	if firstLeadership.Leading() {
		test.Fatal("the deposed leader still believes it leads")
	}
	var term int64
	_ = pool.QueryRow(context.Background(), `select term from leadership`).Scan(&term)
	if term != 2 {
		test.Fatalf("term %d", term)
	}
}
