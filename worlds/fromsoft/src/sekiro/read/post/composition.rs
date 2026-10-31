//! Scalar composition after row sampling and activation have been resolved by the caller.
//!
//! The native consumer at 14017b9a0 visits 21 banks in ascending order. Its scalar accessor at 140326cf0
//! skips inactive rows and copies the incoming sample at full weight. This module does not infer bank activation,
//! sample time keys, or replace an unlisted authored ID with its registered reference initializer.
use fromsoft_formats::Result;

/// A bank's independently resolved activation and blend weight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Activation {
    Inactive,
    /// Weights at or below zero preserve the previous value; weights at or above one copy the incoming value.
    Active {
        weight: f32,
    },
}

/// A field row after native load flags, value-ID selection and time sampling have been resolved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScalarRow {
    Inactive,
    Active(f32),
}

/// One explicit input bank. An absent activation is unresolved, rather than implicitly active.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScalarBank {
    pub index: u8,
    pub activation: Option<Activation>,
    pub row: ScalarRow,
}

/// Compose explicit scalar banks in native order, regardless of the input slice's order.
///
/// `initial` is supplied by the caller, not a renderer default. Unresolved activation (including MapBase's
/// activation/weight), duplicate banks, out-of-range banks and non-finite inputs are refused. Bank zero has a
/// separate native fallback path and must be supplied through `initial`, not as an ordinary active row.
pub fn scalar(initial: f32, banks: &[ScalarBank]) -> Result<f32> {
    let refuse = |index, detail: &str| crate::invalid("GPARAM scalar composition", index, detail);
    if !initial.is_finite() {
        return Err(refuse(0, "non-finite initial scalar"));
    }
    let mut ordered = [None; 21];
    for bank in banks {
        let index = usize::from(bank.index);
        if index == 0 || index >= ordered.len() {
            return Err(refuse(
                index,
                "bank must be in 1..21; bank zero is the initial fallback",
            ));
        }
        if ordered[index].replace(bank).is_some() {
            return Err(refuse(index, "duplicate bank"));
        }
        let Some(activation) = bank.activation else {
            return Err(refuse(
                index,
                if index == 4 {
                    "MapBase activation/weight not traced"
                } else {
                    "bank activation/weight unresolved"
                },
            ));
        };
        if let Activation::Active { weight } = activation
            && !weight.is_finite()
        {
            return Err(refuse(index, "non-finite bank weight"));
        }
        if let ScalarRow::Active(value) = bank.row
            && !value.is_finite()
        {
            return Err(refuse(index, "non-finite sampled scalar"));
        }
    }
    let mut value = initial;
    for bank in ordered.into_iter().flatten() {
        let (Some(Activation::Active { weight }), ScalarRow::Active(incoming)) = (bank.activation, bank.row)
        else {
            continue;
        };
        if weight >= 1.0 || (weight > 0.0 && incoming == value) {
            value = incoming;
        } else if weight > 0.0 {
            value = (1.0 - weight) * value + weight * incoming;
        }
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_banks_require_activation_and_keep_native_boundaries() {
        let mut bank = ScalarBank {
            index: 4,
            activation: None,
            row: ScalarRow::Active(4.0),
        };
        assert!(
            scalar(1.6, &[bank])
                .unwrap_err()
                .to_string()
                .contains("MapBase activation/weight not traced")
        );
        for (weight, expected) in [(-1.0, 2.0), (0.0, 2.0), (0.5, 3.0), (1.0, 4.0), (2.0, 4.0)] {
            bank.activation = Some(Activation::Active { weight });
            assert_eq!(scalar(2.0, &[bank]).unwrap(), expected);
        }
        // The native equality shortcut copies the incoming word before multiplying (140326dde..de5).
        // Multiplying both equal subnormal inputs by one half would underflow and incorrectly produce zero.
        let subnormal = f32::from_bits(1);
        let equal = ScalarBank {
            activation: Some(Activation::Active { weight: 0.5 }),
            row: ScalarRow::Active(subnormal),
            ..bank
        };
        assert_eq!(scalar(subnormal, &[equal]).unwrap().to_bits(), 1);
        assert!(scalar(1.6, &[bank, bank]).is_err());
        for index in [0, 21, u8::MAX] {
            assert!(scalar(1.6, &[ScalarBank { index, ..bank }]).is_err());
        }
        bank.activation = Some(Activation::Active { weight: f32::NAN });
        assert!(scalar(1.6, &[bank]).is_err());
    }
}
