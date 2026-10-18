#!/usr/bin/env python3
"""Create local deployment credentials without overwriting an existing file."""

from __future__ import annotations

import argparse
import base64
import os
from pathlib import Path
import secrets


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path(".env"))
    parser.add_argument("--email", default="admin@example.com")
    args = parser.parse_args()
    if any(character in args.email for character in "\r\n\"' #${}") or "@" not in args.email:
        parser.error("--email must be a valid email without environment-file metacharacters")

    template = Path(__file__).resolve().parents[1] / ".env.example"
    database_password = secrets.token_hex(24)
    values = {
        "POSTGRES_PASSWORD": database_password,
        "CREWDOCK_DATABASE_URL": f"postgres://crewdock:{database_password}@127.0.0.1:5432/crewdock",
        "CREWDOCK_MASTER_KEY": base64.b64encode(secrets.token_bytes(32)).decode("ascii"),
        "CREWDOCK_SESSION_SECRET": secrets.token_hex(32),
        "CREWDOCK_BOOTSTRAP_PASSWORD": secrets.token_urlsafe(24),
        "CREWDOCK_BOOTSTRAP_EMAIL": args.email,
    }
    lines = []
    for line in template.read_text(encoding="utf-8").splitlines():
        key = line.split("=", 1)[0]
        lines.append(f"{key}={values[key]}" if key in values else line)
    try:
        descriptor = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError:
        parser.error(f"{args.output} already exists; refusing to replace credentials")
    with os.fdopen(descriptor, "w", encoding="utf-8", newline="\n") as handle:
        handle.write("\n".join(lines) + "\n")
    print(f"Created {args.output}. Review the public URL, network subnet and administrator email.")
    print("The initial administrator password is stored in CREWDOCK_BOOTSTRAP_PASSWORD in that file.")


if __name__ == "__main__":
    main()
