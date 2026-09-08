# VoteKin

VoteKin is a WebAssembly vote listener for [Pumpkin](https://pumpkinmc.org/). It receives NuVotifier v2 votes, verifies signatures and challenges, and logs the service and player name. Optional legacy Votifier v1 votes are decrypted and logged without sender authentication. Votes aren't stored, delivered to other plugins, or rewarded yet — early development.

## Building

```sh
rustup target add wasm32-wasip2
cargo build-plugin
```

Output: `target/wasm32-wasip2/release/votekin_plugin.wasm`

## Installation

Stop Pumpkin, copy the `.wasm` into `plugins/`, restart, and approve the requested permissions (`network.tcp.bind` to listen for votes; `fs.read.data`/`fs.write.data` for its config folder).

On first load, VoteKin creates `plugins/data/votekin/config.json`:

```json
{
  "bind_address": "0.0.0.0",
  "port": 8192,
  "token": "YOUR_GENERATED_TOKEN",
  "enable_v1": false
}
```

- A secure token is auto-generated and persists across restarts; it's never printed to console — read it from the file, and keep the file private.
- `0.0.0.0` binds all IPv4 interfaces; use `127.0.0.1` for local-only. Must be a valid IPv4/IPv6 address.
- Invalid config blocks loading (and won't be overwritten) — restart Pumpkin after edits.
- Open the configured TCP port in your VPS/provider firewall for external votes.

**Logging:** startup shows the listen address; accepted votes show service + username. Failures log a reason, throttled to one per 5 seconds (with a `suppressed` count of omissions). Tokens, raw packets, and IPs are never logged.

**Limits:** One vote per connection, up to 32 concurrent connections, a five-second deadline per exchange, and an 8 KiB maximum JSON message size.

## Legacy Votifier v1

Votekin also supports older RSA public key voting. The enable this, set `"enable_v1": true`
in `plugins/data/votekin/config.json` and restart Pumpkin. Existing configurations
without this field keep v1 disabled. NuVotifier v2 remains available on the same port.

Once set to true, the first startup generates a 2048-bit RSA key pair. Copy the entire
contents of `plugins/data/votekin/public.key` into the server list's public-key
field.Keep`private.pem` private.

V1 has no token authentication or challenge-based replay protection: anyone
with the public key can submit a vote. Use v2 where available. The RSA library
also has a known [timing-attack advisory](https://rustsec.org/advisories/RUSTSEC-2023-0071.html);
blinded decryption is used, but does not remove that documented limitation.

A v1 packet is exactly 256 encrypted bytes. It receives no success JSON response;
check the console for `Vote received` with `protocol=VotifierV1`. V2 logs show
`protocol=NuVotifierV2`. V1 timestamps have no standard format or units, so VoteKin
records local receipt time without interpreting the supplied timestamp.

## Development checks

```sh
cargo fmt --all --check
cargo test -p votekin-core -p votekin-plugin
cargo clippy -p votekin-core -p votekin-plugin --all-targets -- -D warnings
cargo clippy -p votekin-plugin --target wasm32-wasip2 -- -D warnings
```

## License

MIT
