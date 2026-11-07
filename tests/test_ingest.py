from itertools import pairwise
from pathlib import Path

import pytest

from tellfold.ingest import chunk_text, load_documents

SEED = """# Harbor Town transit plan

The city council of Harbor Town voted on Monday to replace three bus lines with an on-demand shuttle.

Mayor Ines Duarte said the pilot would cut costs by a fifth. The Riders Union called the move a quiet service cut.

Local shop owners on Pier Street worry that fewer fixed stops will mean fewer customers."""


def test_load_documents_reads_text_markdown_and_latin1(tmp_path: Path):
    (tmp_path / "plan.md").write_text(SEED, encoding="utf-8")
    (tmp_path / "notes.txt").write_bytes("Café owners on Pier Street are à bout de patience. " .encode("latin-1") * 20)
    (tmp_path / "empty.txt").write_text("   \n\n  ")
    docs = load_documents([tmp_path / "plan.md", tmp_path / "notes.txt", tmp_path / "empty.txt"])
    assert [name for name, _ in docs] == ["plan.md", "notes.txt"]
    assert "Riders Union" in docs[0][1]
    assert "Café" in docs[1][1]


def test_load_documents_expands_directories_and_rejects_unknown(tmp_path: Path):
    (tmp_path / "a.markdown").write_text("alpha")
    (tmp_path / "b.txt").write_text("beta")
    (tmp_path / "skip.csv").write_text("x,y")
    assert [n for n, _ in load_documents([tmp_path])] == ["a.markdown", "b.txt"]
    with pytest.raises(ValueError):
        load_documents([tmp_path / "skip.csv"])
    with pytest.raises(FileNotFoundError):
        load_documents([tmp_path / "missing.md"])


def test_load_documents_reads_pdf(tmp_path: Path):
    import pymupdf

    path = tmp_path / "brief.pdf"
    with pymupdf.open() as doc:
        doc.new_page().insert_text((72, 72), "Harbor Town shuttle pilot starts in May.")
        doc.save(path)
    [(name, text)] = load_documents([path])
    assert name == "brief.pdf" and "shuttle pilot" in text


def test_chunk_small_doc_is_one_chunk_with_paragraphs_intact():
    chunks = chunk_text([("plan.md", SEED)])
    assert len(chunks) == 1
    assert chunks[0].id == 0 and chunks[0].source == "plan.md"
    assert "\n\n" in chunks[0].text


def test_chunks_respect_size_overlap_and_paragraph_boundaries():
    paragraphs = [f"Paragraph {i} " + "word " * 30 for i in range(12)]
    text = "\n\n".join(p.strip() for p in paragraphs)
    chunks = chunk_text([("a.txt", text), ("b.txt", "short doc")], size=400, overlap=60)
    assert all(len(c.text) <= 400 for c in chunks)
    assert [c.id for c in chunks] == list(range(len(chunks)))
    assert chunks[-1].source == "b.txt" and chunks[-1].text == "short doc"
    a_chunks = [c for c in chunks if c.source == "a.txt"]
    assert len(a_chunks) > 1
    for prev, nxt in pairwise(a_chunks):
        head = nxt.text.split("\n\n")[0]
        assert head and prev.text.endswith(head)  # the overlap is the previous chunk's tail
    for i in range(12):  # every paragraph appears whole in some chunk
        assert any(f"Paragraph {i} word" in c.text for c in a_chunks)


def test_long_paragraph_without_breaks_is_split_by_sentence_and_word():
    text = " ".join(f"Sentence number {i} is here." for i in range(80)) + " " + "x" * 900
    chunks = chunk_text([("long.txt", text)], size=300, overlap=40)
    assert all(len(c.text) <= 300 for c in chunks)
    assert "Sentence number 79 is here." in "".join(c.text for c in chunks)


def test_chunk_rejects_bad_overlap():
    with pytest.raises(ValueError):
        chunk_text([("a", "b")], size=100, overlap=80)
