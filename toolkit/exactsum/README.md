# primase-exactsum

An order-independent sum of finite `f64` terms, kept as non-overlapping partials and rounded once when read,
ties to even. `merge` combines partial sums without losing the result's independence from processing order.
This lets decisions over a map's parts agree with a whole-map sum. Callers must supply finite terms.

## Example

```rust
use primase_exactsum::ExactSum;

fn main() {
    let mut sum = ExactSum::default();
    for term in [1e16, 1.0, -1e16] {
        sum.add(term);
    }
    assert_eq!(sum.value(), 1.0);
}
```

See [CONTRIBUTING](../../CONTRIBUTING.md) for checks and real-data setup.

License: GPL-3.0-or-later.
