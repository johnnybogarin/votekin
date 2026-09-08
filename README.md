# VoteKin

VoteKin is a WebAssembly vote listener for [Pumpkin](https://pumpkinmc.org/). It receives NuVotifier v2 votes, verifies signatures and challenges, and logs the service and player name. Votes aren't stored, delivered to other plugins, or rewarded yet — early development. Votifier v1 isn't supported.

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
  "token": "YOUR_GENERATED_TOKEN"
}
```

- A secure token is auto-generated and persists across restarts; it's never printed to console — read it from the file, and keep the file private.
- `0.0.0.0` binds all IPv4 interfaces; use `127.0.0.1` for local-only. Must be a valid IPv4/IPv6 address.
- Invalid config blocks loading (and won't be overwritten) — restart Pumpkin after edits.
- Open the configured TCP port in your VPS/provider firewall for external votes.

**Logging:** startup shows the listen address; accepted votes show service + username. Failures log a reason, throttled to one per 5 seconds (with a `suppressed` count of omissions). Tokens, raw packets, and IPs are never logged.

**Limits:** one vote per connection, up to 32 concurrent connections, 5s deadline per exchange, 8 KiB max JSON size, non-blocking/tick-bounded socket handling, clean shutdown.

## Development checks

```sh
cargo fmt --all --check
cargo test -p votekin-core -p votekin-plugin
cargo clippy -p votekin-core -p votekin-plugin --all-targets -- -D warnings
cargo clippy -p votekin-plugin --target wasm32-wasip2 -- -D warnings
```

## License

MIT