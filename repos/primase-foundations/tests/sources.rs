//! `SOURCES` lists every source file, so a new file cannot change a consumer's cached output unnoticed.

#[test]
fn sources_list_every_source_file() {
    let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
    let mut files = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                files.push(format!("foundations/{rel}"));
            }
        }
    }
    files.sort();
    let listed: Vec<&str> = primase_foundations::SOURCES.iter().map(|(n, _)| *n).collect();
    assert_eq!(files, listed);
}
