#!/usr/bin/env python3
"""Style ratchet (architecture/STYLE.md): `just style`.

Measures first-party Rust against the conventions in CONTRIBUTING.md and holds
every file to the counts recorded in architecture/style-baseline.json. Like
`just arch`, it is a shrink-only gate: a count that rises, or a file that
starts violating, fails; a count that falls prints a notice so the baseline
can be lowered with --update.

Rules (per file; the limits are the constants below):
  fn_length         functions longer than FN_MAX lines (all code, tests too)
  file_length       the file's line count, recorded only when over FILE_MAX
                    (TEST_FILE_MAX for tests): the baseline pins the size, so
                    a long file cannot keep growing
  process_comments  comment lines naming `Phase N`, `P<digit>` (// comments)
                    or an issue number (`#1234`, `Pulsar-Native#1234`) that is
                    not the form `TODO(#1234): reason`
  println           println!/eprintln! in non-test library code
  unwrap            .unwrap() in non-test code
  pub_result_string `Result<_, String>` in `pub fn` signatures (non-test)

Scope: tracked .rs files (git ls-files; a plain directory walk when --root is
not a git checkout), minus plugins/vendor, crates/renderer/helio,
crates/third-party, crates/ui/wgpui*, crates/external and target/ (separate
repositories or vendored; each audits itself).

"Test code" is a file under tests/ or benches/, named tests.rs, *_tests.rs,
*_test.rs or test_*.rs, declared by `#[cfg(test)] mod x;`, or begins with
`#![cfg(test)]`; inside other files it is the body of any `#[cfg(test)]` or
`#[test]` item. `println` additionally exempts what is not library code:
main.rs, build.rs, src/bin/, examples/, crates that are binaries only (a
main.rs and no lib.rs) and tools/ (CLIs print by design).

The measurement is a lexical approximation, no parser. Comments and string
and char literals are blanked first (nested block comments, raw strings and
lifetimes handled), then `fn` bodies are found by brace matching on what is
left. A function's length is raw lines from its `fn` keyword to its closing
brace, comments and blanks included. Functions produced by macros are measured
as written in the macro body; a brace inside a macro pattern the lexer cannot
see may skew one function. Numbers are for ordering and for the ratchet, where
what matters is that the same code always measures the same.

Usage: python scripts/style-check.py [--root DIR] [--update [--allow-increase]]
       [--report] (just style)

  --update           rewrite the baseline from the tree; refuses when any
                     count would rise or a file would start violating, unless
                     --allow-increase (a pure file move or split: the totals
                     must not rise)
  --report           print totals per rule and the worst files; no gate
"""

import bisect
import json
import os
import re
import subprocess
import sys
from pathlib import Path

FN_MAX = 100
FILE_MAX = 600
TEST_FILE_MAX = 1500

RULES = [
    "fn_length",
    "file_length",
    "process_comments",
    "println",
    "unwrap",
    "pub_result_string",
]

EXCLUDED_PREFIXES = (
    "plugins/vendor/",
    "crates/renderer/helio/",
    "crates/third-party/",
    "crates/ui/wgpui",
    "crates/external/",
    "target/",
)

BASELINE_NAME = "architecture/style-baseline.json"

# ---------------------------------------------------------------- lexing

TOKEN = re.compile(
    r"""//[^\n]*"""
    r"""|/\*"""
    r"""|(?<![A-Za-z0-9_])b?r(?P<hashes>\#*)\""""
    r"""|b?\""""
    r"""|'(?:\\(?:u\{[^}\n]*\}|x[0-9a-fA-F]{2}|[^\n])|[^\\'\n])'"""
)
STRING_END = re.compile(r'(?:[^"\\]|\\.)*"', re.S)
BLOCK_EDGE = re.compile(r"/\*|\*/")
NOT_NEWLINE = re.compile(r"[^\n]")


def blank(text):
    return NOT_NEWLINE.sub(" ", text)


def lex(src):
    """(code, comments): `src` with comments and literal contents blanked to
    spaces (newlines kept, so offsets and lines match), and the comments as
    (offset, text, is_line_comment)."""
    parts = []
    comments = []
    last = 0
    pos = 0
    n = len(src)
    while True:
        m = TOKEN.search(src, pos)
        if not m:
            break
        start = m.start()
        tok = m.group(0)
        if tok.startswith("//"):
            end = m.end()
            comments.append((start, tok, True))
        elif tok == "/*":
            depth = 1
            end = m.end()
            while depth and end < n:
                e = BLOCK_EDGE.search(src, end)
                if not e:
                    end = n
                    break
                depth += 1 if e.group(0) == "/*" else -1
                end = e.end()
            comments.append((start, src[start:end], False))
        elif tok[-1] == '"' and "r" in tok:
            close = '"' + m.group("hashes")
            e = src.find(close, m.end())
            end = n if e < 0 else e + len(close)
        elif tok[-1] == '"':
            e = STRING_END.match(src, m.end())
            end = n if not e else e.end()
        else:  # char literal
            end = m.end()
        parts.append(src[last:start])
        piece = src[start:end]
        if piece.endswith('"') and len(piece) >= 2:
            parts.append('"' + blank(piece[1:-1]) + '"')
        else:
            parts.append(blank(piece))
        last = end
        pos = end
    parts.append(src[last:])
    return "".join(parts), comments


BRACES = re.compile(r"[{}]")


def match_braces(code):
    """{open offset: close offset} over blanked code."""
    stack = []
    pairs = {}
    for m in BRACES.finditer(code):
        if m.group(0) == "{":
            stack.append(m.start())
        elif stack:
            pairs[stack.pop()] = m.start()
    return pairs


FN_DECL = re.compile(r"\bfn\s+([A-Za-z_]\w*)")
BODY_EDGE = re.compile(r"[()\[\]{};]")
TEST_ATTR = re.compile(r"#\[\s*(?:cfg\s*\(\s*test\s*\)|(?:\w+::)*test\b[^\]]*)\]")
OTHER_ATTR = re.compile(r"\s*#\[[^\]]*\]")
CFG_TEST_MOD = re.compile(
    r"#\[\s*cfg\s*\(\s*test\s*\)\s*\]\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;"
)
PUB_PREFIX = re.compile(r'\bpub\s+(?:(?:async|const|unsafe|extern(?:\s+"[^"]*")?)\s+)*$')
PRINTLN = re.compile(r"\b(?:e?println)\s*!")
UNWRAP = re.compile(r"\.\s*unwrap\s*\(\s*\)")


def item_end(code, pos, pairs):
    """Offset after the `{...}` or `;` that ends the item starting at pos."""
    depth = 0
    for m in BODY_EDGE.finditer(code, pos):
        c = m.group(0)
        if c in "([":
            depth += 1
        elif c in ")]":
            depth -= 1
        elif depth <= 0 and c == ";":
            return m.end(), None
        elif depth <= 0 and c == "{":
            close = pairs.get(m.start())
            return (close + 1 if close is not None else None), m.start()
    return None, None


def test_intervals(code, pairs):
    out = []
    for m in TEST_ATTR.finditer(code):
        pos = m.end()
        while True:  # further attributes on the same item
            a = OTHER_ATTR.match(code, pos)
            if not a:
                break
            pos = a.end()
        end, _ = item_end(code, pos, pairs)
        if end is not None:
            out.append((m.start(), end))
    out.sort()
    merged = []
    for s, e in out:
        if merged and s <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(merged[-1][1], e))
        else:
            merged.append((s, e))
    return merged


def in_intervals(starts, intervals, off):
    i = bisect.bisect_right(starts, off) - 1
    return i >= 0 and off < intervals[i][1]


def split_top(text):
    args, depth, cur = [], 0, []
    for ch in text:
        if ch in "<([":
            depth += 1
        elif ch in ">)]":
            depth -= 1
        if ch == "," and depth == 0:
            args.append("".join(cur))
            cur = []
        else:
            cur.append(ch)
    args.append("".join(cur))
    return [a.strip() for a in args if a.strip()]


STRING_TYPES = ("String", "std::string::String", "alloc::string::String")


def returns_result_string(sig):
    sig = sig.replace("->", "--")
    for m in re.finditer(r"\bResult\s*<", sig):
        depth, i = 1, m.end()
        while i < len(sig) and depth:
            depth += {"<": 1, ">": -1}.get(sig[i], 0)
            i += 1
        args = split_top(sig[m.end() : i - 1])
        if len(args) == 2 and args[1] in STRING_TYPES:
            return True
    return False


PHASE = re.compile(r"\bPhase [0-9]")
P_NUM = re.compile(r"\bP[0-9]\b")
ISSUE = re.compile(r"(?<![&])#[0-9]{3,}\b")
TODO_OK = re.compile(r"TODO\(#[0-9]+\):\s*\S")


def process_comment_lines(comments):
    n = 0
    for _, text, is_line in comments:
        for line in text.split("\n"):
            line = TODO_OK.sub("", line)
            if PHASE.search(line) or ISSUE.search(line) or (is_line and P_NUM.search(line)):
                n += 1
    return n


# ---------------------------------------------------------------- scope


def tracked_rs(root):
    try:
        out = subprocess.run(
            ["git", "ls-files", "-z", "--", "*.rs"],
            cwd=root,
            check=True,
            capture_output=True,
        ).stdout.decode("utf-8", "replace")
        if (root / ".git").exists():
            files = [p for p in out.split("\0") if p]
            return sorted(f for f in files if (root / f).is_file())
    except (OSError, subprocess.CalledProcessError):
        pass
    files = []
    for d, dirs, names in os.walk(root):
        dirs[:] = [x for x in dirs if x not in (".git", "target")]
        for name in names:
            if name.endswith(".rs"):
                files.append(Path(d, name).relative_to(root).as_posix())
    return sorted(files)


def first_party(rel):
    return not rel.startswith(EXCLUDED_PREFIXES)


def is_test_path(rel):
    p = rel.split("/")
    stem = p[-1][:-3]
    return (
        "tests" in p[:-1]
        or "benches" in p[:-1]
        or stem == "tests"
        or stem.endswith(("_tests", "_test"))
        or stem.startswith("test_")
    )


def exempt_println(rel, bin_only_dirs):
    p = rel.split("/")
    return (
        p[-1] in ("main.rs", "build.rs")
        or "examples" in p[:-1]
        or "bin" in p[:-1]
        or p[0] == "tools"
        or any(rel.startswith(d + "/") for d in bin_only_dirs)
    )


# ---------------------------------------------------------------- measure


def measure(root):
    files = [f for f in tracked_rs(root) if first_party(f)]
    all_paths = set(files)
    lexed = {}
    for rel in files:
        src = (root / rel).read_text(encoding="utf-8", errors="replace").lstrip("﻿")
        code, comments = lex(src)
        lexed[rel] = (src, code, comments, match_braces(code))

    # Binary-only crates: a main.rs and no lib.rs beside it, so every module
    # under src/ is CLI code.
    bin_only = set()
    for rel in files:
        if rel.endswith("/src/main.rs") and rel[: -len("main.rs")] + "lib.rs" not in all_paths:
            bin_only.add(rel[: -len("/main.rs")])

    # Files declared `#[cfg(test)] mod x;`.
    declared_test = set()
    for rel, (_, code, _, _) in lexed.items():
        base = rel.rsplit("/", 1)[0] if "/" in rel else ""
        name = rel.rsplit("/", 1)[-1]
        own = base if name in ("mod.rs", "lib.rs", "main.rs") else (base + "/" if base else "") + name[:-3]
        for m in CFG_TEST_MOD.finditer(code):
            d = own + "/" if own else ""
            declared_test.add(d + m.group(1) + ".rs")
            declared_test.add(d + m.group(1) + "/mod.rs")

    result = {r: {} for r in RULES}
    for rel, (src, code, comments, pairs) in lexed.items():
        nl = [i for i, c in enumerate(src) if c == "\n"] if False else None
        newlines = [m.start() for m in re.finditer("\n", code)]

        def line_of(off):
            return bisect.bisect_left(newlines, off) + 1

        test_file = (
            is_test_path(rel)
            or rel in declared_test
            or re.match(r"\s*#!\[\s*cfg\s*\(\s*test\s*\)\s*\]", code) is not None
        )
        lines = src.count("\n") + (1 if src and not src.endswith("\n") else 0)
        limit = TEST_FILE_MAX if test_file else FILE_MAX
        if lines > limit:
            result["file_length"][rel] = lines

        intervals = [] if test_file else test_intervals(code, pairs)
        istarts = [s for s, _ in intervals]

        def non_test(off):
            return not test_file and not in_intervals(istarts, intervals, off)

        long_fns = 0
        pub_results = 0
        for m in FN_DECL.finditer(code):
            end, brace = item_end(code, m.end(), pairs)
            if brace is not None and end is not None:
                if line_of(end - 1) - line_of(m.start()) + 1 > FN_MAX:
                    long_fns += 1
            if end is not None and non_test(m.start()):
                if PUB_PREFIX.search(code[max(0, m.start() - 60) : m.start()]) and returns_result_string(
                    code[m.end() : (brace if brace is not None else end)]
                ):
                    pub_results += 1
        if long_fns:
            result["fn_length"][rel] = long_fns
        if pub_results:
            result["pub_result_string"][rel] = pub_results

        n = process_comment_lines(comments)
        if n:
            result["process_comments"][rel] = n

        n = sum(1 for m in UNWRAP.finditer(code) if non_test(m.start()))
        if n:
            result["unwrap"][rel] = n

        if not exempt_println(rel, bin_only):
            n = sum(1 for m in PRINTLN.finditer(code) if non_test(m.start()))
            if n:
                result["println"][rel] = n
    return result


# ---------------------------------------------------------------- gate


def load(path):
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return None
    return {r: dict(data.get("rules", {}).get(r, {})) for r in RULES}


def save(path, rules):
    data = {
        "about": "Shrink-only style baseline (architecture/STYLE.md). Per rule, per file: "
        "the violation count (file_length: the line count). Never edit by hand; "
        "`just style --update` only lowers it.",
        "limits": {"fn_length": FN_MAX, "file_length": FILE_MAX, "test_file_length": TEST_FILE_MAX},
        "rules": {r: dict(sorted(rules[r].items())) for r in RULES},
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        json.dump(data, f, indent=1)
        f.write("\n")


def compare(cur, base):
    """(rises, falls) as lists of (rule, file, was, now)."""
    rises, falls = [], []
    for r in RULES:
        for f in sorted(set(cur[r]) | set(base[r])):
            was, now = base[r].get(f, 0), cur[r].get(f, 0)
            if now > was:
                rises.append((r, f, was, now))
            elif now < was:
                falls.append((r, f, was, now))
    return rises, falls


def totals(rules):
    return {r: (len(rules[r]), sum(rules[r].values())) for r in RULES}


def print_totals(rules):
    for r, (files, total) in totals(rules).items():
        what = "lines in" if r == "file_length" else "violations in"
        if r == "file_length":
            print(f"  {r:18} {files} files over the limit")
        else:
            print(f"  {r:18} {total} {what} {files} files")


def report(rules):
    print_totals(rules)
    for r in RULES:
        worst = sorted(rules[r].items(), key=lambda kv: -kv[1])[:5]
        print(f"\n{r}: worst files")
        for f, c in worst:
            print(f"  {c:6}  {f}")


def main():
    args = sys.argv[1:]
    root = Path(args[args.index("--root") + 1]).resolve() if "--root" in args else Path(
        __file__
    ).resolve().parents[1]
    update = "--update" in args
    allow = "--allow-increase" in args
    baseline_path = root / BASELINE_NAME

    cur = measure(root)
    if "--report" in args:
        report(cur)
        return 0
    base = load(baseline_path)

    if update:
        if base is not None:
            rises, _ = compare(cur, base)
            if rises and not allow:
                print("refusing to raise the baseline (pass --allow-increase only for a pure move or split):")
                for r, f, was, now in rises:
                    print(f"  {r}: {f}: {was} -> {now}")
                return 1
        save(baseline_path, cur)
        print(f"wrote {BASELINE_NAME}")
        print_totals(cur)
        return 0

    if base is None:
        print(f"{BASELINE_NAME} is missing: run `just style --update` to create it")
        return 1

    rises, falls = compare(cur, base)
    if rises:
        print("style: new violations (fix them; the baseline only shrinks):")
        for r, f, was, now in rises:
            note = "new file" if was == 0 else f"was {was}"
            print(f"  {r}: {f}: {now} ({note})")
        print("rules and how to fix each: architecture/STYLE.md")
        return 1
    print("style: ok")
    print_totals(cur)
    if falls:
        print(f"\nbaseline can shrink ({len(falls)} entries improved): run `just style --update`")
        for r, f, was, now in falls[:20]:
            print(f"  {r}: {f}: {was} -> {now}")
        if len(falls) > 20:
            print(f"  ... and {len(falls) - 20} more")
    return 0


if __name__ == "__main__":
    sys.exit(main())
