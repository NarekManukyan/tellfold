"""Seed document loading and paragraph-aware chunking."""

from __future__ import annotations

import re
from pathlib import Path

from charset_normalizer import from_bytes

from shoal.models import Chunk

TEXT_SUFFIXES = {".md", ".markdown", ".txt"}
SUPPORTED_SUFFIXES = TEXT_SUFFIXES | {".pdf"}

_PARAGRAPH_BREAK = re.compile(r"\n\s*\n")
_SENTENCE_END = re.compile(r"(?<=[.!?。！？])\s+")
_JOINER = "\n\n"


def load_documents(paths: list[Path]) -> list[tuple[str, str]]:
    """Read seed files (directories are expanded) into (source name, text) pairs, skipping empty ones."""
    docs: list[tuple[str, str]] = []
    for path in _expand(paths):
        text = _read_pdf(path) if path.suffix.lower() == ".pdf" else _read_text(path)
        text = _normalize(text)
        if text:
            docs.append((path.name, text))
    return docs


def chunk_text(docs: list[tuple[str, str]], size: int = 1800, overlap: int = 200) -> list[Chunk]:
    """Split documents into chunks of at most `size` chars, packing whole paragraphs where possible.

    Consecutive chunks of the same document share up to `overlap` trailing characters so that facts
    straddling a boundary stay readable in at least one chunk. Chunk ids are global and sequential.
    """
    if size <= 0 or not 0 <= overlap < size // 2:
        raise ValueError("chunk size must be positive and overlap smaller than half of it")
    chunks: list[Chunk] = []
    for source, text in docs:
        for body in _pack(_segments(text, size - overlap - len(_JOINER)), size, overlap):
            chunks.append(Chunk(id=len(chunks), source=source, text=body))
    return chunks


def _expand(paths: list[Path]) -> list[Path]:
    files: list[Path] = []
    for path in paths:
        path = Path(path)
        if path.is_dir():
            files.extend(sorted(p for p in path.rglob("*") if p.suffix.lower() in SUPPORTED_SUFFIXES))
        elif not path.exists():
            raise FileNotFoundError(path)
        elif path.suffix.lower() not in SUPPORTED_SUFFIXES:
            raise ValueError(f"unsupported seed file type: {path.name} (use .pdf, .md, .markdown or .txt)")
        else:
            files.append(path)
    return files


def _read_pdf(path: Path) -> str:
    import pymupdf

    with pymupdf.open(path) as doc:
        return _JOINER.join(page.get_text() for page in doc)


def _read_text(path: Path) -> str:
    raw = path.read_bytes()
    try:
        return raw.decode("utf-8-sig")
    except UnicodeDecodeError:
        best = from_bytes(raw).best()
        return str(best) if best is not None else raw.decode("utf-8", errors="replace")


def _normalize(text: str) -> str:
    text = text.replace("\r\n", "\n").replace("\r", "\n").replace("\x00", "")
    lines = [re.sub(r"[ \t\f\v]+", " ", line).strip() for line in text.split("\n")]
    return re.sub(r"\n{3,}", _JOINER, "\n".join(lines)).strip()


def _segments(text: str, limit: int) -> list[str]:
    """Paragraphs no longer than `limit`; long paragraphs are split by sentence, then by word."""
    out: list[str] = []
    for para in _PARAGRAPH_BREAK.split(text):
        para = para.strip()
        if not para:
            continue
        if len(para) <= limit:
            out.append(para)
            continue
        out.extend(_greedy_join(_SENTENCE_END.split(para), limit, " "))
    return out


def _greedy_join(pieces: list[str], limit: int, sep: str) -> list[str]:
    out: list[str] = []
    current = ""
    for piece in pieces:
        for part in _hard_split(piece, limit):
            candidate = f"{current}{sep}{part}" if current else part
            if len(candidate) <= limit:
                current = candidate
            else:
                out.append(current)
                current = part
    if current:
        out.append(current)
    return out


def _hard_split(piece: str, limit: int) -> list[str]:
    parts: list[str] = []
    while len(piece) > limit:
        cut = piece.rfind(" ", 0, limit + 1)
        cut = cut if cut > limit // 2 else limit
        parts.append(piece[:cut].strip())
        piece = piece[cut:].strip()
    if piece:
        parts.append(piece)
    return parts


def _pack(segments: list[str], size: int, overlap: int) -> list[str]:
    chunks: list[str] = []
    current = ""
    for seg in segments:
        candidate = f"{current}{_JOINER}{seg}" if current else seg
        if len(candidate) <= size:
            current = candidate
            continue
        chunks.append(current)
        tail = _tail(current, overlap)
        current = f"{tail}{_JOINER}{seg}" if tail else seg
    if current:
        chunks.append(current)
    return chunks


def _tail(text: str, overlap: int) -> str:
    """Last <= `overlap` chars of `text`, starting at a word boundary."""
    if overlap == 0:
        return ""
    tail = text[-overlap:]
    if len(text) > overlap:
        space = re.search(r"\s", tail)
        tail = tail[space.end():] if space else tail
    return tail.strip()
