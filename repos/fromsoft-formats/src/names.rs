//! The case key of FromSoftware resource names, as the archive dictionary and binder lookup compare them. Stored names
//! remain unchanged for reporting.

/// Fold ASCII letters as the archive dictionary and binder lookup do. This does not strip a path,
/// normalise separators, remove extensions, or case-fold Unicode: those are separate operations.
pub fn fold(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// The case key of a resource path's file name without its last extension: `N:\Tex\Rock_A.tif`'s is `rock_a`.
pub fn stem(path: &str) -> String {
    let leaf = path.rsplit(['/', '\\']).next().unwrap_or(path);
    fold(leaf.rsplit_once('.').map_or(leaf, |x| x.0))
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
