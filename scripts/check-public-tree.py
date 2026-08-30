#!/usr/bin/env python3
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
EXCLUDED = {".git", "target", "build-output", "dist", ".cargo"}
TEXT_SUFFIXES = {
    ".c",
    ".h",
    ".html",
    ".json",
    ".md",
    ".py",
    ".rc",
    ".rs",
    ".sh",
    ".toml",
    ".txt",
    ".yaml",
    ".yml",
}
FORBIDDEN = [
    "179" + ".253.232.199",
    "aig" + ".hanabi-ai.cn",
    "BEGIN " + "PRIVATE KEY",
]
RAW_TOKEN = re.compile(r"WHALE_READ_TOKEN=[A-Za-z0-9_-]{40,}")


def main():
    findings = []
    for path in ROOT.rglob("*"):
        if not path.is_file() or any(part in EXCLUDED for part in path.relative_to(ROOT).parts):
            continue
        if path.suffix.lower() not in TEXT_SUFFIXES:
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        relative = path.relative_to(ROOT)
        for forbidden in FORBIDDEN:
            if forbidden in text:
                findings.append(f"{relative}: contains forbidden deployment-specific text")
        if RAW_TOKEN.search(text):
            findings.append(f"{relative}: contains a raw Whale token")
    if findings:
        print("Public-tree scan failed:", file=sys.stderr)
        for finding in findings:
            print(f"- {finding}", file=sys.stderr)
        raise SystemExit(1)
    print("Public-tree scan passed")


if __name__ == "__main__":
    main()
