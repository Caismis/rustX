#!/usr/bin/env python3
"""Reproducible production/test line counts for the Issue #487 size report.

Counting method (physical lines, comments and blanks included):

* Rust: a file reached only through a `#[cfg(test)] mod x;` declaration (and
  every module beneath it) is test code, as is everything under `tests/`.
  Inside other files, every item annotated `#[cfg(test)]` (from the attribute
  to the item's closing brace or semicolon) is counted as test; all remaining
  lines are production.
* TypeScript: files under `test/`, `tests/`, or named `*.test.ts(x)` /
  `*.spec.ts(x)` are test; `*.generated.ts` and `generated/` are generated;
  everything else under `src/` is production.

Usage: measure.py [--json] PATH...   (files or directories, relative to the
repository root). Prints `production test generated path` per file.
"""
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def strip_code(line, state):
    """Return the line with comments and literals blanked; track block
    comments and raw/ordinary strings that span lines in `state`."""
    out = []
    i = 0
    n = len(line)
    while i < n:
        if state.get("block"):
            end = line.find("*/", i)
            if end < 0:
                return "".join(out)
            state["block"] = False
            i = end + 2
            continue
        if state.get("raw") is not None:
            term = '"' + "#" * state["raw"]
            end = line.find(term, i)
            if end < 0:
                return "".join(out)
            state["raw"] = None
            i = end + len(term)
            continue
        if state.get("str"):
            while i < n:
                c = line[i]
                if c == "\\":
                    i += 2
                    continue
                if c == '"':
                    state["str"] = False
                    i += 1
                    break
                i += 1
            continue
        c = line[i]
        if line.startswith("//", i):
            break
        if line.startswith("/*", i):
            state["block"] = True
            i += 2
            continue
        m = re.match(r'b?r(#*)"', line[i:])
        if m and (i == 0 or not (line[i - 1].isalnum() or line[i - 1] == "_")):
            state["raw"] = len(m.group(1))
            i += m.end()
            continue
        if c == '"':
            state["str"] = True
            i += 1
            continue
        if c == "'":
            m = re.match(r"'(\\.|[^\\'])'|'\\u\{[0-9a-fA-F]+\}'|'\\x[0-9a-fA-F]{2}'", line[i:])
            if m:
                i += m.end()
                continue
        out.append(c)
        i += 1
    return "".join(out)


def rust_split(path):
    """Return (production, test, test_child_modules) for one Rust file."""
    with open(path, encoding="utf-8") as fh:
        lines = fh.read().split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    state = {}
    code = [strip_code(line, state) for line in lines]
    test_lines = 0
    test_children = []
    i = 0
    pending_attr_start = None
    while i < len(lines):
        stripped = code[i].strip()
        if re.match(r"#\[cfg\((all\()?test\b", stripped) and "not(test" not in stripped:
            start = i
            # Find the annotated item: skip further attributes.
            j = i
            depth = 0
            item_started = False
            while j < len(lines):
                text = code[j]
                if j == start:
                    text = text[text.find("]") + 1:] if "]" in text else ""
                for ch in text:
                    if ch == "{":
                        depth += 1
                        item_started = True
                    elif ch == "}":
                        depth -= 1
                    elif ch == ";" and depth == 0 and text.strip() and not text.strip().startswith("#"):
                        item_started = True
                        depth = -1
                        break
                m = re.match(r"\s*(pub(\([^)]*\))?\s+)?mod\s+(\w+)\s*;", code[j])
                if m and j >= start:
                    test_children.append(m.group(3))
                if item_started and depth <= 0:
                    break
                j += 1
            test_lines += j - start + 1
            i = j + 1
            continue
        i += 1
    return len(lines) - test_lines, test_lines, test_children


def rust_module_dir(path):
    base = os.path.basename(path)
    d = os.path.dirname(path)
    if base in ("mod.rs", "lib.rs", "main.rs"):
        return d
    return os.path.join(d, base[:-3])


def rust_child_file(path, name):
    d = rust_module_dir(path)
    for candidate in (os.path.join(d, name + ".rs"), os.path.join(d, name, "mod.rs")):
        if os.path.exists(candidate):
            return candidate
    return None


def rust_test_files():
    """Every src/ Rust file that is only compiled under cfg(test)."""
    result = set()
    for dirpath, _, files in os.walk(os.path.join(ROOT, "src")):
        for f in files:
            if f.endswith(".rs"):
                p = os.path.join(dirpath, f)
                _, _, children = rust_split(p)
                for name in children:
                    child = rust_child_file(p, name)
                    if child:
                        result.add(child)
    # Descendants of test-only files are test-only too.
    changed = True
    while changed:
        changed = False
        for dirpath, _, files in os.walk(os.path.join(ROOT, "src")):
            for f in files:
                p = os.path.join(dirpath, f)
                if p in result or not f.endswith(".rs"):
                    continue
                parent_dir = os.path.dirname(p)
                for t in list(result):
                    if parent_dir == rust_module_dir(t) or parent_dir.startswith(rust_module_dir(t) + os.sep):
                        result.add(p)
                        changed = True
                        break
    return result


def ts_kind(rel):
    parts = rel.split("/")
    base = parts[-1]
    if "generated" in parts or ".generated." in base:
        return "generated"
    if "test" in parts or "tests" in parts or re.search(r"\.(test|spec)\.tsx?$", base):
        return "test"
    return "production"


def measure(paths):
    rust_tests = rust_test_files()
    rows = []
    for arg in paths:
        full = os.path.join(ROOT, arg)
        files = []
        if os.path.isdir(full):
            for dirpath, dirs, fs in os.walk(full):
                dirs[:] = [d for d in dirs if d != "node_modules"]
                files.extend(os.path.join(dirpath, f) for f in fs)
        elif os.path.exists(full):
            files.append(full)
        for p in sorted(files):
            rel = os.path.relpath(p, ROOT)
            if p.endswith(".rs"):
                if p in rust_tests or rel.startswith("tests/"):
                    with open(p, encoding="utf-8") as fh:
                        total = fh.read().count("\n")
                    rows.append((0, total, 0, rel))
                else:
                    prod, test, _ = rust_split(p)
                    rows.append((prod, test, 0, rel))
            elif re.search(r"\.tsx?$", p):
                with open(p, encoding="utf-8") as fh:
                    total = fh.read().count("\n")
                kind = ts_kind(rel)
                rows.append((
                    total if kind == "production" else 0,
                    total if kind == "test" else 0,
                    total if kind == "generated" else 0,
                    rel,
                ))
    return rows


def main():
    args = sys.argv[1:]
    as_json = "--json" in args
    args = [a for a in args if a != "--json"]
    rows = measure(args)
    if as_json:
        print(json.dumps([dict(production=a, test=b, generated=c, path=d) for a, b, c, d in rows]))
        return
    for a, b, c, d in rows:
        print(f"{a:7d} {b:7d} {c:7d} {d}")
    print(f"{sum(r[0] for r in rows):7d} {sum(r[1] for r in rows):7d} {sum(r[2] for r in rows):7d} TOTAL")


if __name__ == "__main__":
    main()
