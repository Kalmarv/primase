# primase-cache

An opt-in content-addressed disk cache for expensive pure operations. Keys hash tagged, length-prefixed fields;
callers include the operation version, source hash and encoder versions. `Cache` checks each entry's key, length
and payload digest, writes through temporary files and trims least recently used entries to the caller's cap.
The program supplies the directory; the library reads no environment.

## Example

```rust
use primase_cache::KeyBuilder;

fn main() {
    let key = KeyBuilder::new("image-operation")
        .u32("version", 1)
        .bytes("pixels", &[255, 0, 0, 255])
        .finish();
    assert_eq!(key.hex().len(), 64);
    println!("{}", key.hex());
}
```

See [CONTRIBUTING](../../CONTRIBUTING.md) for checks and real-data setup.

License: GPL-3.0-or-later.
