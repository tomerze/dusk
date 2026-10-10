const Redis = require("ioredis");

const [address, prefix] = process.argv.slice(2);
const [host, port] = address.split(":");
const key = (name) => prefix + name;
const failures = [];

function expect(what, got, want) {
  if (JSON.stringify(got) === JSON.stringify(want)) {
    console.log(`ok   ${what}`);
    return;
  }
  failures.push(what);
  console.log(`FAIL ${what}: got ${JSON.stringify(got)}, want ${JSON.stringify(want)}`);
}

async function refusal(promise) {
  try {
    await promise;
    return "no error";
  } catch (error) {
    return error.message;
  }
}

async function main() {
  const client = new Redis({ host, port: Number(port), maxRetriesPerRequest: 0 });

  expect("PING", await client.ping(), "PONG");
  expect("PING message", await client.call("PING", "hello"), "hello");

  expect("SET", await client.set(key("one"), "1"), "OK");
  expect("GET", await client.get(key("one")), "1");
  expect("GET absent", await client.get(key("missing")), null);

  expect("MSET", await client.mset(key("two"), "2", key("three"), "3"), "OK");
  expect("MGET", await client.mget(key("one"), key("two"), key("missing")), ["1", "2", null]);

  expect("EXISTS", await client.exists(key("one"), key("two"), key("missing")), 2);

  expect("TYPE string", await client.type(key("one")), "string");
  expect("TYPE none", await client.type(key("missing")), "none");
  expect("TYPE list", await client.type("logs.overwritten"), "list");
  expect("GET list", (await refusal(client.get("logs.overwritten"))).startsWith("WRONGTYPE"), true);

  expect("GET unsigned integer", /^[0-9]+$/.test(await client.get("dusk.namespace_id")), true);

  expect("KEYS", (await client.keys(key("*"))).sort(), [key("one"), key("three"), key("two")]);

  const scanned = [];
  let cursor = "0";
  do {
    const [next, page] = await client.scan(cursor, "MATCH", key("*"), "COUNT", 2);
    scanned.push(...page);
    cursor = next;
  } while (cursor !== "0");
  expect("SCAN", scanned.sort(), [key("one"), key("three"), key("two")]);

  expect("SET sticky", await client.set("dusk.hostname", "redis-" + prefix), "OK");
  expect("GET sticky", await client.get("dusk.hostname"), "redis-" + prefix);

  expect("SET EX", (await refusal(client.set(key("expiring"), "v", "EX", 10))).includes("not supported"), true);

  expect("DEL", await client.del(key("one"), key("two"), key("missing")), 2);
  expect("EXISTS deleted", await client.exists(key("one")), 0);

  expect("QUIT", await client.quit(), "OK");

  if (failures.length > 0) {
    console.log(`${failures.length} failed`);
    process.exit(1);
  }
}

main().catch((error) => {
  console.log(`FAIL ${error.stack}`);
  process.exit(1);
});
