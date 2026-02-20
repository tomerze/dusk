// Generate random 64-bit hex IDs for capnp schemas
// Called as a pre-hook by cargo-generate

fn random_u64_hex() {
    // Read 8 bytes from /dev/urandom and format as 0x-prefixed hex
    let bytes = system::command("od", ["-A", "n", "-t", "x8", "-N", "8", "/dev/urandom"]);
    // od output has leading whitespace, strip it and take first token
    let hex = bytes.trim().split(" ")[0].trim();
    `0x${hex}`
}

variable::set("capnp-file-id", `@${random_u64_hex()}`);
variable::set("program-id",    random_u64_hex());
variable::set("result-type-id", random_u64_hex());
