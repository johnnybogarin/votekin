# VoteKin

VoteKin is a WebAssembly vote listener for [Pumpkin](https://pumpkinmc.org/). It can receive NuVotifier v2 votes or optional (config enabled) legacy Votifier v1 votes. Received votes are forwarded live to subscribed Pumpkin plugins. VoteKin does not store votes or issue rewards.

## Building

```sh
rustup target add wasm32-wasip2
cargo build-plugin
```

Output: `target/wasm32-wasip2/release/VoteKin.wasm`

## Installation

Stop Pumpkin, copy `VoteKin.wasm` into `plugins/`, restart. Approve the requested permissions (`network.tcp.bind` to listen for votes; `fs.read.data`/`fs.write.data` for its config folder).

On first load, VoteKin creates `plugins/data/votekin/config.json`:

```json
{
  "bind_address": "0.0.0.0",
  "port": 8192,
  "token": "YOUR_GENERATED_TOKEN",
  "enable_v1": false
}
```

- A secure token is auto-generated at first load as the `token` value. This is persisted. Keep the token private.
- `0.0.0.0` binds all IPv4 interfaces; use `127.0.0.1` for local-only. Must be a valid IPv4/IPv6 address.
- Invalid config blocks loading (and won't be overwritten) — restart Pumpkin after edits.

**Logging:** startup shows the listen address; accepted votes show service + username. Failures log a reason, throttled to one per 5 seconds (with a `suppressed` count of omissions). Tokens, raw packets, and IPs are never logged.

**Limits:** One vote per connection, up to 32 concurrent connections, a five-second deadline per exchange, and an 8 KiB maximum JSON message size.

## Legacy Votifier v1 (Not really recommended)

Votekin supports older RSA public key voting. The enable this, set `"enable_v1": true`
in `plugins/data/votekin/config.json` and restart Pumpkin. Existing configurations
without this field keep v1 disabled. NuVotifier v2 remains available on the same port.

Once set to true and after start up, copy the entire
contents of `plugins/data/votekin/public.key` into the server list's public-key
field. Keep`private.pem` private.

NOTE: V1 has no token authentication or challenge-based replay protection: anyone
with the public key can submit a vote. Ideally, use v2 if available. The RSA library
also has a known [timing-attack advisory](https://rustsec.org/advisories/RUSTSEC-2023-0071.html);
blinded decryption is used, but does not remove that documented limitation.

A v1 packet is 256 encrypted bytes. It receives no success JSON response, so check the console for `Vote received` with `protocol=VotifierV1`. V2 logs show
`protocol=NuVotifierV2`. V1 timestamps have no standard format or units, so VoteKin
records local receipt time without interpreting the supplied timestamp (duno if thats useful).

## Plugin integration (IPC v1)

Declare `votekin` as a plugin dependency. Send JSON bytes via
`ipc::send_ipc_message` to `votekin` on load/unload:

```json
{"type":"votekin:subscribe/v1"}
{"type":"votekin:unsubscribe/v1"}
```

Subscriptions use Pumpkin's sender identity. Both requests are idempotent,
limited to 256 bytes, and return `{"status":"ok"}` or an IPC error.

Handle votes in `handle_ipc_message`, verifying the sender is `votekin`:

```json
{
  "type": "votekin:vote.accepted/v1",
  "service": "MinecraftIndex.com",
  "username": "Alex",
  "source_protocol": "nuvotifier_v2",
  "voted_at": 1788890856000,
  "received_at": 1788890856123
}
```

`source_protocol` is `nuvotifier_v2` or `votifier_v1`; v1 does not authenticate
senders. Timestamps are signed Unix milliseconds: `voted_at` is sender-provided
(`null` for v1), and `received_at` is local. Return `Ok(Vec::new())` on success
or `Err(...)` on failure.

Delivery is synchronous and live only, with no storage or retries. Consumer
errors are logged without stopping other deliveries or removing subscriptions.
Subscription changes affect subsequent votes; resubscribe after VoteKin reloads.
A v2 acknowledgement confirms validation, not reward delivery.

## Development checks

```sh
cargo fmt --all --check
cargo test -p votekin-core -p votekin
cargo clippy -p votekin-core -p votekin --all-targets -- -D warnings
cargo clippy -p votekin --target wasm32-wasip2 -- -D warnings
```

## Examples

This plugin is currently being used by Planetmine, where they have voting enabled at [MinecraftIndex](https://www.minecraftindex.com/server/kiQO38CzqJj4jM-KD9Xg1). 


## License

MIT
