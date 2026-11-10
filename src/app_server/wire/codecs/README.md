# Generated numeric codec descriptors

These private MessagePack assets contain only the compact plan compiled from each
closed envelope's Rust-owned `schemars::JsonSchema`. They are generated data, not
another handwritten numeric schema. Runtime admission, encoding and raw request
parameter checks share the same immutable descriptor. Full schema export still
uses the Rust types on demand.

After changing an envelope or its reachable DTOs, regenerate with:

```sh
cargo test --locked --lib regenerate_native_numeric_codec -- --ignored
cargo test --locked --lib app_server::wire
```

Ordinary tests compile each native Rust schema again and compare the descriptor
bytes exactly, so stale assets fail CI. Numeric rules live in the shared plan
compiler and value codec. This is a build artifact format; it is not persisted in
conversation stores and has no compatibility or migration path.
