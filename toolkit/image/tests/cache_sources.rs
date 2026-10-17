/// `CACHE_SOURCES` lists every source file, so a new file cannot change cached output unnoticed.
#[test]
fn cache_sources_list_every_source_file() {
    let mut files: Vec<String> = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))
        .unwrap()
        .map(|e| format!("image/{}", e.unwrap().file_name().to_string_lossy()))
        .collect();
    files.sort();
    let listed: Vec<&str> = primase_image::CACHE_SOURCES.iter().map(|(n, _)| *n).collect();
    assert_eq!(files, listed);
}
