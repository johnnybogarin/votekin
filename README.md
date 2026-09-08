# VoteKin

VoteKin is a WebAssembly vote listener for [Pumpkin](https://pumpkinmc.org/). It receives NuVotifier v2 votes, verifies signatures and challenges, and logs the service and player name. Optional legacy Votifier v1 votes are decrypted and logged without sender authentication. Received votes are forwarded live to subscribed Pumpkin plugins. VoteKin does not store votes or issue rewards.

## Building

```sh
rustup target add wasm32-wasip2
cargo build-plugin
```

Output: `target/wasm32-wasip2/release/VoteKin.wasm`

## Installation

Stop Pumpkin, copy `VoteKin.wasm` into `plugins/`, restart, and approve the requested permissions (`network.tcp.bind` to listen for votes; `fs.read.data`/`fs.write.data` for its config folder).

On first load, VoteKin creates `plugins/data/votekin/config.json`:

```json
{
  "bind_address": "0.0.0.0",
  "port": 8192,
  "token": "YOUR_GENERATED_TOKEN",
  "enable_v1": false
}
```

- A secure token is auto-generated at first load as the `token` value, which persists across restarts; it's never printed to console. Keep the file/token private.
- `0.0.0.0` binds all IPv4 interfaces; use `127.0.0.1` for local-only. Must be a valid IPv4/IPv6 address.
- Invalid config blocks loading (and won't be overwritten) — restart Pumpkin after edits.

**Logging:** startup shows the listen address; accepted votes show service + username. Failures log a reason, throttled to one per 5 seconds (with a `suppressed` count of omissions). Tokens, raw packets, and IPs are never logged.

**Limits:** One vote per connection, up to 32 concurrent connections, a five-second deadline per exchange, and an 8 KiB maximum JSON message size.

## Legacy Votifier v1

Votekin also supports older RSA public key voting. The enable this, set `"enable_v1": true`
in `plugins/data/votekin/config.json` and restart Pumpkin. Existing configurations
without this field keep v1 disabled. NuVotifier v2 remains available on the same port.

Once set to true and after start up, copy the entire
contents of `plugins/data/votekin/public.key` into the server list's public-key
field.Keep`private.pem` private.

V1 has no token authentication or challenge-based replay protection: anyone
with the public key can submit a vote. Ideally, use v2 if available. The RSA library
also has a known [timing-attack advisory](https://rustsec.org/advisories/RUSTSEC-2023-0071.html);
blinded decryption is used, but does not remove that documented limitation.

A v1 packet is 256 encrypted bytes. It receives no success JSON response, so check the console for `Vote received` with `protocol=VotifierV1`. V2 logs show
`protocol=NuVotifierV2`. V1 timestamps have no standard format or units, so VoteKin
records local receipt time without interpreting the supplied timestamp (duno if thats useful).

## Plugin integration (IPC v1)

Consumers (plugins that offer vote rewards for example) should declare `votekin` as a Pumpkin plugin dependency so it loads
first. Send UTF-8 JSON bytes to plugin `votekin` with Pumpkin's
`ipc::send_ipc_message` during consumer load:

```json
{"type":"votekin:subscribe/v1"}
```

VoteKin registers the sender identity supplied by Pumpkin. There is no recipient
field. To unsubscribe, send `{"type":"votekin:unsubscribe/v1"}` during consumer
unload. Both requests are idempotent and return `{"status":"ok"}` as JSON bytes.
Malformed or unsupported requests return an IPC error. Requests are limited to
256 bytes. Check both result layers: `Ok(Ok(response))` means VoteKin returned a
response; `Ok(Err(reason))` is a handler error and `Err(())` is a host delivery failure.

Consumers implement Pumpkin's `handle_ipc_message` callback. Check that the
host-supplied sender is `votekin`, then read messages with this shape:

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

- `source_protocol` is `nuvotifier_v2` or `votifier_v1`. An accepted v1 vote is
  decrypted and validated, but its sender is not authenticated.
- Timestamps are signed integer Unix milliseconds. `voted_at` is the sender's
  claimed time, or `null` for v1. `received_at` is recorded locally by VoteKin.
- No token, address, private key, or raw packet is included.
- Return `Ok(Vec::new())` after handling a vote. VoteKin ignores successful
  response bytes. Return `Err(...)` to report a delivery failure.

Delivery is synchronous and uses a snapshot of subscribers for each vote.
Subscription changes made during a callback affect subsequent votes. Keep
callbacks short; slow consumers delay processing. A returned error or unavailable
consumer is logged and does not stop attempts to the remaining subscribers.
Failed subscriptions remain registered until explicitly removed or VoteKin unloads.

There is no vote storage, retry, replay, or reward-delivery guarantee. Votes sent
with no subscribers are only logged; missed votes cannot be recovered. A v2
success response confirms vote validation, not consumer processing or rewards.
Subscriptions are cleared when VoteKin unloads. Consumers must subscribe again
after VoteKin reloads; restarting Pumpkin together is the simplest approach.

## Development checks

```sh
cargo fmt --all --check
cargo test -p votekin-core -p votekin
cargo clippy -p votekin-core -p votekin --all-targets -- -D warnings
cargo clippy -p votekin --target wasm32-wasip2 -- -D warnings
```

## License

MIT
