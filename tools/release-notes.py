# SPDX-License-Identifier: Apache-2.0
"""Drafts a hexcavator release's notes from the pull requests merged since
the release before it (for hexcavator-release.yml, which publishes them;
edit them on the release afterwards if they need it).

    python tools/release-notes.py hexcavator-v0.0.12 > notes.md
    python tools/release-notes.py --self-test

The pull requests are those squash-merged between the previous
`hexcavator-v*` tag and this one (their commits end with `(#N)`) that
change what hexcavator is made of: apps/hexcavator, its extension
(editors/vscode/hexcavator), its web version (editors/web), or a crate it
links (cargo metadata says which). Each is its title, linked, and the
first paragraph of its description; grouped as commands, decoders, the
program's size, and the rest. Needs git (with the tags), cargo, and gh
(GH_TOKEN, to read the pull requests).
"""

import json
import os
import re
import subprocess
import sys

REPO = os.environ.get("GH_REPO", "turbonop/turbonop-utilities")

# The paths of hexcavator's own parts, besides the crates it links.
OWN = ("apps/hexcavator/", "editors/vscode/hexcavator/", "editors/web/")

# The instruction sets' decoders, by the name a title starts with.
DECODERS = (
    "arm32", "arm64", "x86", "x64", "mips", "riscv", "powerpc", "loongarch",
    "tablegen", "wasm",
)


def run(*args):
    return subprocess.run(args, check=True, capture_output=True, text=True).stdout


def version(tag):
    """`hexcavator-v0.0.11` as (0, 0, 11), for ordering."""
    return tuple(int(p) for p in re.findall(r"\d+", tag.split("-v", 1)[-1]))


def previous_tag(tag, tags):
    """The newest `hexcavator-v*` tag older than `tag`, if any."""
    older = [t for t in tags if t.startswith("hexcavator-v") and version(t) < version(tag)]
    return max(older, key=version) if older else None


def pull_numbers(subjects):
    """The pull requests squash-merged, by their commits' subjects."""
    out = []
    for s in subjects:
        m = re.search(r"\(#(\d+)\)\s*$", s)
        if m:
            out.append(int(m.group(1)))
    return out


def linked_dirs(metadata):
    """The directories (relative, with a trailing slash) of the workspace
    crates hexcavator links: its normal dependencies, transitively."""
    root = metadata["workspace_root"].replace("\\", "/").rstrip("/") + "/"
    by_id = {p["id"]: p for p in metadata["packages"]}
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    start = next(p["id"] for p in metadata["packages"] if p["name"] == "hexcavator")
    seen, todo = set(), [start]
    while todo:
        i = todo.pop()
        if i in seen:
            continue
        seen.add(i)
        for d in nodes[i]["deps"]:
            if any(k.get("kind") in (None, "normal") for k in d["dep_kinds"]):
                todo.append(d["pkg"])
    dirs = []
    for i in seen:
        path = by_id[i]["manifest_path"].replace("\\", "/")
        if path.startswith(root):
            dirs.append(path[len(root):].rsplit("/", 1)[0] + "/")
    return sorted(dirs)


def relevant(files, dirs):
    """Whether a pull request's files change hexcavator."""
    return any(f.startswith(OWN) or any(f.startswith(d) for d in dirs) for f in files)


def is_release(title):
    """Whether a pull request is a release's own (`Release hexcavator
    0.0.11`: its version, bumped)."""
    return bool(re.match(r"Release[ :]", title))


def first_paragraph(body):
    """A description's first paragraph, on one line (not a heading, a
    list, a table, or a quote)."""
    for para in re.split(r"\n\s*\n", (body or "").replace("\r\n", "\n")):
        text = para.strip()
        if not text or text.startswith(("#", "|", ">", "```", "<!--")):
            continue
        if re.match(r"^([-*]|\d+\.)\s", text):
            continue
        return " ".join(line.strip() for line in text.split("\n"))
    return ""


def group(title):
    """The section a pull request goes in, by its title."""
    head = title.split(":", 1)[0].strip().lower() if ":" in title else ""
    if re.search(r"\bsmaller\b", title, re.I):
        return "Smaller"
    if head.startswith(("hexcavator ", "hexc ")) or head in ("hexcavator", "hexc"):
        return "Commands"
    if any(head.startswith(d) for d in DECODERS):
        return "Decoders"
    return "Other changes"


SECTIONS = [
    ("Commands", "### Commands"),
    ("Decoders", "### Instruction decoding"),
    ("Smaller", "### A smaller program"),
    ("Other changes", "### Other changes"),
]


def notes(tag, prev, prs):
    """The notes: `prs` is [(number, title, body)], in merge order."""
    name = tag.split("-v", 1)[-1]
    out = [f"## What's new in hexcavator {name}", ""]
    if not prs:
        out += ["No changes to hexcavator since the last release.", ""]
    grouped = {}
    for number, title, body in prs:
        grouped.setdefault(group(title), []).append((number, title, body))
    for key, heading in SECTIONS:
        items = grouped.get(key)
        if not items:
            continue
        out += [heading, ""]
        for number, title, body in items:
            line = f"- **{title}** ([#{number}](https://github.com/{REPO}/pull/{number}))"
            para = first_paragraph(body)
            if para:
                line += f": {para}"
            out.append(line)
        out.append("")
    base = prev or ""
    if prev:
        out.append(f"**Full changelog**: https://github.com/{REPO}/compare/{base}...{tag}")
    else:
        out.append(f"**Full changelog**: https://github.com/{REPO}/commits/{tag}")
    return "\n".join(out) + "\n"


def main(tag):
    tags = run("git", "tag", "--list", "hexcavator-v*").split()
    prev = previous_tag(tag, tags)
    span = f"{prev}..{tag}" if prev else tag
    subjects = run("git", "log", "--format=%s", "--reverse", span).splitlines()
    metadata = json.loads(run("cargo", "metadata", "--format-version", "1", "--locked"))
    dirs = linked_dirs(metadata)
    prs = []
    for n in pull_numbers(subjects):
        pr = json.loads(run("gh", "pr", "view", str(n), "--repo", REPO, "--json", "title,body,files"))
        files = [f["path"] for f in pr.get("files", [])]
        # (Not the releases' own: their version bumps.)
        if relevant(files, dirs) and not is_release(pr["title"]):
            # (Titles end without their number: the link has it.)
            title = re.sub(r"\s*\(#\d+\)$", "", pr["title"])
            prs.append((n, title, pr.get("body", "")))
    sys.stdout.write(notes(tag, prev, prs))


def self_test():
    assert version("hexcavator-v0.0.11") == (0, 0, 11)
    tags = ["hexcavator-v0.0.9", "hexcavator-v0.0.10", "hexcavator-v0.0.11", "stackrabbit-v0.1.0"]
    assert previous_tag("hexcavator-v0.0.11", tags) == "hexcavator-v0.0.10"
    assert previous_tag("hexcavator-v0.0.12", tags) == "hexcavator-v0.0.11"
    assert previous_tag("hexcavator-v0.0.9", tags) is None
    assert pull_numbers(["a (#1)", "b", "c (#23) "]) == [1, 23]
    dirs = ["crates/isa/arm64/", "crates/analysis/size/"]
    assert relevant(["apps/hexcavator/src/lib.rs"], dirs)
    assert relevant(["crates/isa/arm64/build.rs", "TODO.md"], dirs)
    assert not relevant(["TODO.md", "docs/turbolib.md"], dirs)
    assert not relevant(["crates/link/fold/src/lib.rs", "apps/turbolib/src/main.rs"], dirs)
    body = "## Heading\n\nFirst paragraph,\nwrapped.\n\n- a list\n\nSecond."
    assert first_paragraph(body) == "First paragraph, wrapped."
    assert first_paragraph("- only\n- a list") == ""
    assert is_release("Release hexcavator 0.0.11") and is_release("Release 0.0.6: x")
    assert not is_release("Releases published at once")
    assert group("hexcavator size: unnamed data is its object's") == "Commands"
    assert group("arm64: SMSTART decodes") == "Decoders"
    assert group("regex: Unicode tables (158 KB smaller)") == "Smaller"
    assert group("Packaging: winget from the release") == "Other changes"
    text = notes("hexcavator-v0.0.12", "hexcavator-v0.0.11", [
        (57, "hexcavator size: unnamed data is its object's", "On a PE image, ...\n\nMore."),
        (58, "regex: fewer tables (158 KB smaller)", ""),
    ])
    assert "## What's new in hexcavator 0.0.12" in text
    assert "### Commands\n\n- **hexcavator size: unnamed data is its object's** ([#57](" in text
    assert "): On a PE image, ..." in text
    assert "### A smaller program" in text and "### Instruction decoding" not in text
    assert text.endswith("compare/hexcavator-v0.0.11...hexcavator-v0.0.12\n")
    metadata = {
        "workspace_root": "/w",
        "packages": [
            {"id": "h", "name": "hexcavator", "manifest_path": "/w/apps/hexcavator/Cargo.toml"},
            {"id": "a", "name": "turbonop-arm64", "manifest_path": "/w/crates/isa/arm64/Cargo.toml"},
            {"id": "t", "name": "turbonop-test", "manifest_path": "/w/crates/test/Cargo.toml"},
            {"id": "r", "name": "regex", "manifest_path": "/registry/regex/Cargo.toml"},
        ],
        "resolve": {"nodes": [
            {"id": "h", "deps": [
                {"pkg": "a", "dep_kinds": [{"kind": None}]},
                {"pkg": "t", "dep_kinds": [{"kind": "dev"}]},
                {"pkg": "r", "dep_kinds": [{"kind": None}]},
            ]},
            {"id": "a", "deps": []}, {"id": "t", "deps": []}, {"id": "r", "deps": []},
        ]},
    }
    assert linked_dirs(metadata) == ["apps/hexcavator/", "crates/isa/arm64/"]
    print("release-notes: self-test passed")


if __name__ == "__main__":
    if sys.argv[1:] == ["--self-test"]:
        self_test()
    elif len(sys.argv) == 2:
        main(sys.argv[1])
    else:
        sys.exit(__doc__)
