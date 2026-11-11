#!/usr/bin/env python3
"""Split REVIEW_NOTES.md into one task file per numbered finding.

    python3 tools/split_review_notes.py [--notes REVIEW_NOTES.md] [--out .claude/bugs]

A finding is a heading like `### BUG 13.11: title`, `### 9.5: title` or
`### PERF / CLEANUP 9.7`. Section 13, finding 11 becomes `1311.md`; 1.1 becomes
`0101.md`. Unnumbered blocks ("Small things", "No findings", ...) are not tasks
and are skipped.

Each file is a self-contained prompt for a small model: the task rules for that
kind of finding, the section context, the finding verbatim, and every finding it
cross-references (inlined under "Related", one level deep).
"""
import argparse
import re
import sys
from pathlib import Path

KINDS = "BUG|RISK|PERF|CLEANUP|IDEA"
FINDING = re.compile(
    rf"^### (?P<kind>(?:{KINDS})(?: / (?:{KINDS}))*)?\s*(?P<sec>\d+)\.(?P<n>\d+)\b(?P<rest>.*)$"
)
SECTION = re.compile(r"^## (?P<sec>\d+)\. (?P<title>.*)$")
HEADING = re.compile(r"^#{2,3} ")
REF = re.compile(rf"\b(?:(?:{KINDS}) )?(\d{{1,2}})\.(\d{{1,2}})\b")
UNSURE = re.compile(r"suspected|not checked|not confirmed|read, not run|unverified", re.I)

RULES = {
    "BUG": (
        "Behaviour the reviewer believes is wrong.",
        "1. Open the cited code and confirm the described path really exists as written "
        "(line numbers may have drifted; search by function name).\n"
        "2. If confirmed, apply the smallest fix that matches the note's suggested fix, "
        "and add or extend a unit test when the code is testable without a GPU.\n"
        "3. If the note's claim does not hold, change nothing and say why.",
    ),
    "RISK": (
        "Correct today but fragile, or wrong only under conditions the note names.",
        "1. Confirm the condition in the code.\n"
        "2. Make the narrowest change that removes the fragility (a guard, a clamp, "
        "an explicit error). Do not redesign.\n"
        "3. If the note says \"suspected\" or \"not checked\" and you cannot confirm it by "
        "reading the code, change nothing and report what you found.",
    ),
    "PERF": (
        "A cost the reviewer measured or expects.",
        "1. Confirm the hot path in the code.\n"
        "2. Implement the note's smallest suggested fix only. Behaviour must be identical.\n"
        "3. If the fix needs a wide refactor (many files, a new type threaded through "
        "the app), do not start it: write down what you would do and stop.",
    ),
    "CLEANUP": (
        "Simpler or clearer code, no behaviour change.",
        "1. Make only the cleanup described; behaviour must not change.\n"
        "2. If it touches more than about five files or needs a new abstraction, "
        "do not start: write down a plan and stop.",
    ),
    "IDEA": (
        "A behaviour worth adding or changing; nothing is broken.",
        "Do NOT implement. Write a short design note (where the change would go, the "
        "smallest version, open questions) as your final answer.",
    ),
}

COMMON = """\
## Project rules (apply to every task)

- RawEditor is a Rust app (iced 0.13, wgpu, WGSL). Read `CLAUDE.md` first; its
  constraints (no blocking the UI thread, uniform struct layout, one tone-map stage,
  UK spelling "colour" in UI text) override anything in the finding.
- Never run `cargo fmt`: the repo is not rustfmt-clean and it would rewrite unrelated code.
- Verify with `cargo build --release` or `cargo check`, then
  `cargo clippy --all-targets -- -D warnings` (must stay at zero) and `cargo test <name>`
  for tests you touched. Do not run the ignored GPU tests.
- Change only what this finding needs. Do not fix other findings you notice; mention
  them in your answer instead.
- Do NOT commit, push, tag or edit `REVIEW_NOTES.md`.
- The finding is a reviewer's note, written from reading the code. It may be wrong or
  out of date. Trust the code over the note.

## Your final answer

Reply with: (1) CONFIRMED / NOT CONFIRMED / PARTLY, with one line of evidence;
(2) the files you changed, or "none"; (3) the build, clippy and test results;
(4) anything left undone or unsure.
"""


def parse(text):
    """Return ({sec: (title, intro)}, {(sec, n): entry dict})."""
    sections, entries = {}, {}
    cur_sec = None
    cur = None  # dict being filled
    in_fence = False
    for line in text.splitlines():
        if line.lstrip().startswith("```"):
            in_fence = not in_fence
        heading = (not in_fence) and HEADING.match(line)
        if heading:
            cur = None
            m = SECTION.match(line)
            if m:
                cur_sec = int(m["sec"])
                sections[cur_sec] = [m["title"].strip(), []]
                continue
            m = FINDING.match(line)
            if m:
                key = (int(m["sec"]), int(m["n"]))
                cur = {
                    "sec": key[0], "n": key[1],
                    "kind": (m["kind"] or "").strip(),
                    "heading": line[4:].strip(),
                    "title": m["rest"].strip(" :"),
                    "body": [],
                }
                entries[key] = cur
                continue
            continue  # an unnumbered ### block: not a task, drop its text
        if cur is not None:
            cur["body"].append(line)
        elif cur_sec is not None and not heading and entries.get((cur_sec, 1)) is None:
            # text between a section heading and its first finding
            sections[cur_sec][1].append(line)
    for e in entries.values():
        e["body"] = "\n".join(e["body"]).strip("\n")
    return sections, entries


def kind_of(e):
    first = (e["kind"].split("/")[0].strip()) if e["kind"] else ""
    if first in RULES:
        return first
    # `### 9.5:` has no kind; infer a conservative one from the text
    return "RISK"


def related(e, entries):
    out = []
    own = (e["sec"], e["n"])
    seen = {own}
    for m in REF.finditer(e["heading"] + "\n" + e["body"]):
        key = (int(m[1]), int(m[2]))
        if key in entries and key not in seen:
            seen.add(key)
            out.append(entries[key])
    return out


def render(e, sections, entries, notes_name):
    kind = kind_of(e)
    what, steps = RULES[kind]
    sec_title, intro = sections.get(e["sec"], ("", []))
    intro = "\n".join(intro).strip()
    ident = f"{e['sec']}.{e['n']}"
    unsure = kind != "IDEA" and bool(UNSURE.search(e["heading"] + e["body"]))
    rel = related(e, entries)

    p = []
    p.append("---")
    p.append(f"id: \"{e['sec']:02d}{e['n']:02d}\"")
    p.append(f"finding: \"{ident}\"")
    p.append(f"kind: {kind}")
    p.append(f"section: \"{e['sec']}. {sec_title}\"")
    p.append(f"unverified: {str(unsure).lower()}")
    p.append("---\n")
    p.append(f"# Task {ident}: {e['title'] or e['heading']}\n")
    p.append(
        f"You are working on one finding from a code review of RawEditor "
        f"(`{notes_name}`, finding {ident}). Kind: **{kind}**. {what}\n"
    )
    p.append("## What to do\n")
    p.append(steps + "\n")
    if unsure:
        p.append(
            "This finding is marked suspected / not checked / read-not-run. "
            "Confirming it comes first; do not change code you cannot tie to the described "
            "failure.\n"
        )
    p.append(COMMON if kind != "IDEA" else COMMON.split("## Your final answer")[0]
             + "## Your final answer\n\nThe design note only. Do not change any file.\n")
    if intro:
        p.append(f"## Section context ({e['sec']}. {sec_title})\n")
        p.append(intro + "\n")
    p.append(f"## The finding\n")
    p.append(f"### {e['heading']}\n")
    p.append(e["body"] + "\n")
    if rel:
        p.append("## Related findings (context only; do not act on these)\n")
        for r in rel:
            p.append(f"### {r['heading']}\n")
            p.append(r["body"] + "\n")
    return "\n".join(p).rstrip() + "\n"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--notes", default="REVIEW_NOTES.md")
    ap.add_argument("--out", default=".claude/bugs")
    args = ap.parse_args()

    notes = Path(args.notes)
    if not notes.is_file():
        sys.exit(f"{notes} not found (run from the repo root)")
    sections, entries = parse(notes.read_text(encoding="utf-8"))
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    for stale in out.glob("[0-9][0-9][0-9][0-9].md"):
        stale.unlink()
    for key in sorted(entries):
        e = entries[key]
        if key[0] > 99 or key[1] > 99:
            print(f"skip {key}: does not fit XXYY", file=sys.stderr)
            continue
        name = f"{key[0]:02d}{key[1]:02d}.md"
        (out / name).write_text(render(e, sections, entries, notes.name), encoding="utf-8")
    print(f"wrote {len(entries)} files to {out}/")


if __name__ == "__main__":
    main()
