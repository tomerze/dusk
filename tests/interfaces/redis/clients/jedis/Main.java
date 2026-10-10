import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.Objects;
import java.util.TreeSet;
import redis.clients.jedis.Jedis;
import redis.clients.jedis.Protocol;
import redis.clients.jedis.commands.ProtocolCommand;
import redis.clients.jedis.params.ScanParams;
import redis.clients.jedis.params.SetParams;
import redis.clients.jedis.resps.ScanResult;
import redis.clients.jedis.util.SafeEncoder;

public class Main {
    static final List<String> failures = new ArrayList<>();

    static void expect(String what, Object got, Object want) {
        if (Objects.equals(got, want)) {
            System.out.println("ok   " + what);
            return;
        }
        failures.add(what);
        System.out.println("FAIL " + what + ": got " + got + ", want " + want);
    }

    static String refusal(Runnable command) {
        try {
            command.run();
            return "no error";
        } catch (RuntimeException error) {
            return String.valueOf(error.getMessage());
        }
    }

    public static void main(String[] arguments) {
        String[] address = arguments[0].split(":");
        String prefix = arguments[1];
        try (Jedis client = new Jedis(address[0], Integer.parseInt(address[1]))) {
            expect("PING", client.ping(), "PONG");
            expect("PING message", SafeEncoder.encode((byte[]) client.sendCommand(Protocol.Command.PING, "hello")), "hello");

            expect("SET", client.set(prefix + "one", "1"), "OK");
            expect("GET", client.get(prefix + "one"), "1");
            expect("GET absent", client.get(prefix + "missing"), null);

            expect("MSET", client.mset(prefix + "two", "2", prefix + "three", "3"), "OK");
            expect("MGET", client.mget(prefix + "one", prefix + "two", prefix + "missing"), Arrays.asList("1", "2", null));

            expect("EXISTS", client.exists(prefix + "one", prefix + "two", prefix + "missing"), 2L);

            expect("TYPE string", client.type(prefix + "one"), "string");
            expect("TYPE none", client.type(prefix + "missing"), "none");
            expect("TYPE list", client.type("logs.overwritten"), "list");
            expect("GET list", refusal(() -> client.get("logs.overwritten")).startsWith("WRONGTYPE"), true);

            expect("GET unsigned integer", client.get("dusk.namespace_id").matches("[0-9]+"), true);

            expect("KEYS", new TreeSet<>(client.keys(prefix + "*")), new TreeSet<>(List.of(prefix + "one", prefix + "three", prefix + "two")));

            List<String> scanned = new ArrayList<>();
            String cursor = ScanParams.SCAN_POINTER_START;
            do {
                ScanResult<String> page = client.scan(cursor, new ScanParams().match(prefix + "*").count(2));
                scanned.addAll(page.getResult());
                cursor = page.getCursor();
            } while (!cursor.equals(ScanParams.SCAN_POINTER_START));
            Collections.sort(scanned);
            expect("SCAN", scanned, List.of(prefix + "one", prefix + "three", prefix + "two"));

            expect("SET sticky", client.set("dusk.hostname", "redis-" + prefix), "OK");
            expect("GET sticky", client.get("dusk.hostname"), "redis-" + prefix);

            expect("SET EX", refusal(() -> client.set(prefix + "expiring", "v", SetParams.setParams().ex(10))).contains("not supported"), true);

            expect("DEL", client.del(prefix + "one", prefix + "two", prefix + "missing"), 2L);
            expect("EXISTS deleted", client.exists(prefix + "one"), false);

            ProtocolCommand quit = () -> SafeEncoder.encode("QUIT");
            expect("QUIT", SafeEncoder.encode((byte[]) client.sendCommand(quit, new String[0])), "OK");
        }
        if (!failures.isEmpty()) {
            System.out.println(failures.size() + " failed");
            System.exit(1);
        }
    }
}
