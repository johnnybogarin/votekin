# VoteKin

VoteKin is a vote listener for [Pumpkin](https://pumpkinmc.org/). It is built as
a WebAssembly plugin and will support NuVotifier v2 and legacy Votifier v1 vote
services.

VoteKin is in early development and does not accept votes yet.

## Building

Install Rust and the WebAssembly target:

```sh
rustup target add wasm32-wasip2
```

Build the plugin:

```sh
cargo build-plugin
```

The plugin will be written to
`target/wasm32-wasip2/release/votekin_plugin.wasm`.

## License

VoteKin is available under the MIT License.
