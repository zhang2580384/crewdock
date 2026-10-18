#!/usr/bin/env python3
"""Check tracked publication files for private artifacts and obvious credentials."""

from pathlib import Path
import re
import subprocess


def main() -> None:
    root = Path(__file__).resolve().parents[1]
    tracked = subprocess.check_output(["git", "ls-files", "-z"], cwd=root).decode("utf-8").split("\0")
    forbidden_parts = {".agents", ".codex", ".claude", ".worktrees", "node_modules", "target", "dist", "tmp", "temp", "backups", "__pycache__"}
    forbidden_suffixes = {".db", ".sqlite", ".sqlite3", ".dump", ".pem", ".key", ".p12", ".pfx", ".log", ".zip", ".tgz", ".rdp"}
    secret_pattern = re.compile(r"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----|\bgh[pousr]_[A-Za-z0-9]{30,}|\bAKIA[0-9A-Z]{16}\b|\bsk-[A-Za-z0-9_-]{40,}")
    findings = []
    for name in filter(None, tracked):
        path = Path(name)
        private_env = path.name.startswith(".env") and path.name != ".env.example"
        runtime_path = name.startswith(("deploy/data/", "deploy/files/", "deploy/update-control/", "exports/"))
        if private_env or runtime_path or forbidden_parts.intersection(path.parts) or path.suffix.lower() in forbidden_suffixes or name.endswith(".tar.gz") or path.name.lower().startswith("bookmarks"):
            findings.append(f"Private or generated artifact: {name}")
            continue
        data = (root / name).read_bytes()
        try:
            content = data.decode("utf-8")
        except UnicodeDecodeError:
            continue
        for number, line in enumerate(content.splitlines(), 1):
            if secret_pattern.search(line):
                findings.append(f"Possible credential: {name}:{number}")

    template = (root / ".env.example").read_text(encoding="utf-8")
    for key in ("POSTGRES_PASSWORD", "CREWDOCK_MASTER_KEY", "CREWDOCK_SESSION_SECRET", "CREWDOCK_BOOTSTRAP_PASSWORD"):
        if not re.search(rf"^{key}=$", template, re.MULTILINE):
            findings.append(f"Environment template must leave {key} empty")
    if findings:
        raise SystemExit("\n".join(findings))
    print(f"Publication audit passed for {len(list(filter(None, tracked)))} tracked files.")


if __name__ == "__main__":
    main()
