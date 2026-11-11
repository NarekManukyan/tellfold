//! Port of tests/test_ingest.py.

use std::fs;

use tellfold_llm::ingest::{chunk_text, chunk_text_default, load_documents, IngestError};

const SEED: &str = "# Harbor Town transit plan

The city council of Harbor Town voted on Monday to replace three bus lines with an on-demand shuttle.

Mayor Ines Duarte said the pilot would cut costs by a fifth. The Riders Union called the move a quiet service cut.

Local shop owners on Pier Street worry that fewer fixed stops will mean fewer customers.";

fn latin1(s: &str) -> Vec<u8> {
    s.chars().map(|c| u8::try_from(c as u32).unwrap()).collect()
}

#[test]
fn load_documents_reads_text_markdown_and_latin1() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    fs::write(d.join("plan.md"), SEED).unwrap();
    fs::write(d.join("notes.txt"), latin1(&"Café owners on Pier Street are à bout de patience. ".repeat(20))).unwrap();
    fs::write(d.join("empty.txt"), "   \n\n  ").unwrap();
    let docs = load_documents(&[d.join("plan.md"), d.join("notes.txt"), d.join("empty.txt")]).unwrap();
    assert_eq!(docs.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["plan.md", "notes.txt"]);
    assert!(docs[0].1.contains("Riders Union"));
    assert!(docs[1].1.contains("Café"));
}

#[test]
fn load_documents_expands_directories_and_rejects_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    fs::write(d.join("a.markdown"), "alpha").unwrap();
    fs::write(d.join("b.txt"), "beta").unwrap();
    fs::write(d.join("skip.csv"), "x,y").unwrap();
    fs::create_dir(d.join("sub")).unwrap();
    fs::write(d.join("sub/c.MD"), "gamma").unwrap();
    let names: Vec<String> = load_documents(&[d.to_path_buf()]).unwrap().into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["a.markdown", "b.txt", "c.MD"]);
    assert!(matches!(load_documents(&[d.join("skip.csv")]), Err(IngestError::Unsupported(_))));
    assert!(matches!(load_documents(&[d.join("missing.md")]), Err(IngestError::NotFound(_))));
}

#[test]
fn directories_skip_symlinked_folders_like_rglob() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("seeds");
    fs::create_dir_all(d.join("sub")).unwrap();
    fs::write(d.join("sub/a.md"), "alpha").unwrap();
    std::os::unix::fs::symlink(d.join("sub"), d.join("linked")).unwrap();
    std::os::unix::fs::symlink(&d, d.join("sub/loop")).unwrap();
    let names: Vec<String> = load_documents(&[d.clone()]).unwrap().into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["a.md"]);
}

#[test]
fn a_broken_pdf_is_unreadable_not_a_panic() {
    let tmp = tempfile::tempdir().unwrap();
    for (name, body) in [
        ("empty.pdf", &b""[..]),
        ("truncated.pdf", b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog"),
        ("no-root.pdf", b"%PDF-1.4\ntrailer\n<< /Size 1 >>\n%%EOF\n"),
    ] {
        let path = tmp.path().join(name);
        fs::write(&path, body).unwrap();
        match load_documents(&[path]) {
            Err(IngestError::Unreadable { name: got, .. }) => assert_eq!(got, name),
            other => panic!("{name}: {other:?}"),
        }
    }
}

#[test]
fn chunk_small_doc_is_one_chunk_with_paragraphs_intact() {
    let chunks = chunk_text_default(&[("plan.md".into(), SEED.into())]);
    assert_eq!(chunks.len(), 1);
    assert_eq!((chunks[0].id, chunks[0].source.as_str()), (0, "plan.md"));
    assert!(chunks[0].text.contains("\n\n"));
}

#[test]
fn chunks_respect_size_overlap_and_paragraph_boundaries() {
    let text = (0..12)
        .map(|i| format!("Paragraph {i} {}", "word ".repeat(30)).trim().to_string())
        .collect::<Vec<_>>()
        .join("\n\n");
    let chunks = chunk_text(&[("a.txt".into(), text), ("b.txt".into(), "short doc".into())], 400, 60).unwrap();
    assert!(chunks.iter().all(|c| c.text.chars().count() <= 400));
    assert_eq!(chunks.iter().map(|c| c.id).collect::<Vec<_>>(), (0..chunks.len() as i64).collect::<Vec<_>>());
    let last = chunks.last().unwrap();
    assert_eq!((last.source.as_str(), last.text.as_str()), ("b.txt", "short doc"));
    let a: Vec<_> = chunks.iter().filter(|c| c.source == "a.txt").collect();
    assert!(a.len() > 1);
    for pair in a.windows(2) {
        let head = pair[1].text.split("\n\n").next().unwrap();
        assert!(!head.is_empty() && pair[0].text.ends_with(head)); // the overlap is the previous chunk's tail
    }
    for i in 0..12 {
        assert!(a.iter().any(|c| c.text.contains(&format!("Paragraph {i} word"))));
    }
}

#[test]
fn long_paragraph_without_breaks_is_split_by_sentence_and_word() {
    let text =
        (0..80).map(|i| format!("Sentence number {i} is here.")).collect::<Vec<_>>().join(" ") + " " + &"x".repeat(900);
    let chunks = chunk_text(&[("long.txt".into(), text)], 300, 40).unwrap();
    assert!(chunks.iter().all(|c| c.text.chars().count() <= 300));
    assert!(chunks.iter().map(|c| c.text.as_str()).collect::<String>().contains("Sentence number 79 is here."));
}

#[test]
fn chunk_rejects_bad_overlap() {
    assert!(matches!(chunk_text(&[("a".into(), "b".into())], 100, 80), Err(IngestError::BadChunking)));
}
