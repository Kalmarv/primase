//! The case key for FromSoftware resource names. Stored names remain unchanged for reporting.

/// Fold ASCII letters as the archive dictionary and binder lookup do. This does not strip a path,
/// normalise separators, remove extensions, or case-fold Unicode: those are separate operations.
pub(crate) fn fold(name: &str) -> String {
    name.to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::fold;

    #[test]
    fn resource_case_keys_fold_only_ascii_and_preserve_path_structure() {
        assert_eq!(fold(r"N:\Character_AMSN.SPX"), r"n:\character_amsn.spx");
        assert_eq!(fold("É/Ä_A"), "É/Ä_a");
        assert_eq!(fold(&fold("Character_AMSN")), fold("Character_AMSN"));
    }
}
