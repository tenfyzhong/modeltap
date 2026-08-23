#!/usr/bin/env python3
import re
import sys
from pathlib import Path


def set_version(version: str, root: Path = Path(".")) -> None:
    cargo_toml = root / "Cargo.toml"
    cargo_lock = root / "Cargo.lock"

    if cargo_toml.exists():
        content = cargo_toml.read_text()
        new_content = re.sub(
            r"^version = \"[^\"]*\"",
            f'version = "{version}"',
            content,
            count=1,
            flags=re.MULTILINE,
        )
        cargo_toml.write_text(new_content)

    if cargo_lock.exists():
        content = cargo_lock.read_text()
        new_content = re.sub(
            r'(name = "modeltap"\nversion = )"[^"]*"',
            f'\\g<1>"{version}"',
            content,
            count=1,
        )
        cargo_lock.write_text(new_content)


def main() -> None:
    if len(sys.argv) < 2 or not sys.argv[1].strip():
        print("Usage: set-version.py <version>", file=sys.stderr)
        sys.exit(1)

    version = sys.argv[1].strip().lstrip("v")
    set_version(version)


if __name__ == "__main__":
    main()
