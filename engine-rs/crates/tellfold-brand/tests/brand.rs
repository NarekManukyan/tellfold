//! The committed logo files are what the geometry builds, byte for byte: edit the generator, never the outputs.

use tellfold_brand::{brand_dir, files};

#[test]
fn every_brand_file_is_what_the_generator_writes() {
    let root = brand_dir();
    let source = std::fs::read_to_string(root.join("src/wordmark-source.svg")).unwrap();
    let files = files(&source).unwrap();
    assert_eq!(files.len(), 20);
    for (name, text) in files {
        let committed = std::fs::read_to_string(root.join(&name)).unwrap();
        assert!(committed == text, "brand/{name} differs from what tellfold-brand writes");
    }
}
