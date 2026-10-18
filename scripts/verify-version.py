#!/usr/bin/env python3
"""Verify release versions across manifests and generated lockfiles."""

import argparse
import json
from pathlib import Path
import re
import tomllib


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    frontend = json.loads((root / "frontend/package.json").read_text(encoding="utf-8"))
    frontend_lock = json.loads((root / "frontend/package-lock.json").read_text(encoding="utf-8"))
    backend = tomllib.loads((root / "backend/Cargo.toml").read_text(encoding="utf-8"))["package"]
    backend_lock = tomllib.loads((root / "Cargo.lock").read_text(encoding="utf-8"))
    locked_package = next(package for package in backend_lock["package"] if package["name"] == backend["name"])
    versions = {
        "frontend/package.json": frontend["version"],
        "frontend/package-lock.json": frontend_lock["version"],
        "frontend/package-lock.json root": frontend_lock["packages"][""]["version"],
        "backend/Cargo.toml": backend["version"],
        "Cargo.lock": locked_package["version"],
    }
    if len(set(versions.values())) != 1:
        raise SystemExit(f"Version mismatch: {versions}")
    version = frontend["version"]
    if not re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", version):
        raise SystemExit(f"Invalid release SemVer: {version}")
    if args.tag and args.tag != f"v{version}":
        raise SystemExit(f"Tag {args.tag} does not match v{version}")
    print(f"CrewDock version check passed: v{version}")


if __name__ == "__main__":
    main()
