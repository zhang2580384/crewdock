# Repository workflow

The canonical public repository is https://github.com/zhang2580384/crewdock.
Use `main` as the default branch and short-lived feature branches for changes.

- Preserve unrelated local changes and start from the latest remote state.
- Never commit private deployments, saved sites, databases, uploads, credentials,
  `.env` files, build output, caches, or local editor/agent state.
- Keep `.env.example` as a placeholder-only template.
- Run frontend tests/build, backend tests, updater tests, and the publication audit
  appropriate to a change. Do not claim runtime checks from compilation alone.
- For a release, synchronize `frontend/package.json`, `backend/Cargo.toml` and
  both lockfiles, commit, create an annotated `vX.Y.Z` tag, then push `main` and
  that tag to `origin`. Never force-push a published release.
- Preserve the AGPL-3.0-or-later license and contributor notices.
