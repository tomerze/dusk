package main

import (
	"context"
	"errors"
	"fmt"
	"os"
	"sort"
	"strconv"
	"strings"

	"github.com/redis/go-redis/v9"
)

var failures []string

func expect(what string, got, want any) {
	if fmt.Sprint(got) == fmt.Sprint(want) {
		fmt.Printf("ok   %s\n", what)
		return
	}
	failures = append(failures, what)
	fmt.Printf("FAIL %s: got %v, want %v\n", what, got, want)
}

func main() {
	address, prefix := os.Args[1], os.Args[2]
	background := context.Background()
	client := redis.NewClient(&redis.Options{Addr: address})
	key := func(name string) string { return prefix + name }

	pong, failure := client.Ping(background).Result()
	expect("PING", fmt.Sprintf("%v %v", pong, failure), "PONG <nil>")
	echo, failure := client.Do(background, "PING", "hello").Text()
	expect("PING message", fmt.Sprintf("%v %v", echo, failure), "hello <nil>")

	set, failure := client.Set(background, key("one"), "1", 0).Result()
	expect("SET", fmt.Sprintf("%v %v", set, failure), "OK <nil>")
	value, failure := client.Get(background, key("one")).Result()
	expect("GET", fmt.Sprintf("%v %v", value, failure), "1 <nil>")
	_, failure = client.Get(background, key("missing")).Result()
	expect("GET absent", errors.Is(failure, redis.Nil), true)

	mset, failure := client.MSet(background, key("two"), "2", key("three"), "3").Result()
	expect("MSET", fmt.Sprintf("%v %v", mset, failure), "OK <nil>")
	values, failure := client.MGet(background, key("one"), key("two"), key("missing")).Result()
	expect("MGET", fmt.Sprintf("%v %v", values, failure), "[1 2 <nil>] <nil>")

	exists, failure := client.Exists(background, key("one"), key("two"), key("missing")).Result()
	expect("EXISTS", fmt.Sprintf("%v %v", exists, failure), "2 <nil>")

	kind, failure := client.Type(background, key("one")).Result()
	expect("TYPE string", fmt.Sprintf("%v %v", kind, failure), "string <nil>")
	kind, failure = client.Type(background, key("missing")).Result()
	expect("TYPE none", fmt.Sprintf("%v %v", kind, failure), "none <nil>")
	kind, failure = client.Type(background, "logs.overwritten").Result()
	expect("TYPE list", fmt.Sprintf("%v %v", kind, failure), "list <nil>")
	_, failure = client.Get(background, "logs.overwritten").Result()
	expect("GET list", failure != nil && strings.HasPrefix(failure.Error(), "WRONGTYPE"), true)

	namespace, failure := client.Get(background, "dusk.namespace_id").Result()
	_, parseError := strconv.ParseUint(namespace, 10, 64)
	expect("GET unsigned integer", fmt.Sprint(failure, parseError), "<nil> <nil>")

	keys, failure := client.Keys(background, key("*")).Result()
	sort.Strings(keys)
	expect("KEYS", fmt.Sprintf("%v %v", keys, failure), fmt.Sprint([]string{key("one"), key("three"), key("two")}, " <nil>"))

	var scanned []string
	var cursor uint64
	for {
		page, next, failure := client.Scan(background, cursor, key("*"), 2).Result()
		if failure != nil {
			expect("SCAN", failure, nil)
			break
		}
		scanned = append(scanned, page...)
		if next == 0 {
			break
		}
		cursor = next
	}
	sort.Strings(scanned)
	expect("SCAN", scanned, []string{key("one"), key("three"), key("two")})

	set, failure = client.Set(background, "dusk.hostname", "redis-"+prefix, 0).Result()
	expect("SET sticky", fmt.Sprintf("%v %v", set, failure), "OK <nil>")
	value, failure = client.Get(background, "dusk.hostname").Result()
	expect("GET sticky", fmt.Sprintf("%v %v", value, failure), "redis-"+prefix+" <nil>")

	failure = client.Set(background, key("expiring"), "v", 10_000_000_000).Err()
	expect("SET EX", failure != nil && strings.Contains(failure.Error(), "not supported"), true)

	deleted, failure := client.Del(background, key("one"), key("two"), key("missing")).Result()
	expect("DEL", fmt.Sprintf("%v %v", deleted, failure), "2 <nil>")
	exists, failure = client.Exists(background, key("one")).Result()
	expect("EXISTS deleted", fmt.Sprintf("%v %v", exists, failure), "0 <nil>")

	quit, failure := client.Do(background, "QUIT").Text()
	expect("QUIT", fmt.Sprintf("%v %v", quit, failure), "OK <nil>")

	if len(failures) > 0 {
		fmt.Printf("%d failed\n", len(failures))
		os.Exit(1)
	}
}
