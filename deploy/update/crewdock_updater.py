#!/usr/bin/env python3
"""Host-side crewdock updater. It intentionally runs outside the API container."""

from __future__ import annotations

import argparse
import contextlib
import json
import os
import re
import shutil
import subprocess
import sys
import time
import urllib.request
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterator

try:
    import fcntl
except ImportError:  # pragma: no cover - Linux-only runtime, Windows runs unit tests.
    fcntl = None


REPOSITORY_URL = "https://github.com/zhang2580384/crewdock.git"
RELEASE_TAG = re.compile(r"^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
ROLLBACK_BACKUP = re.compile(
    r"^pre-v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)-(?:\d{8}-\d{6}|[0-9a-f]{7,64})$"
)
ACTIVE_STATES = {
    "checking",
    "backing_up",
    "downloading",
    "building",
    "deploying",
    "verifying",
    "rolling_back",
}


class UpdateError(RuntimeError):
    pass


def utc_now() -> str:
    return datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def version_tuple(tag: str) -> tuple[int, int, int]:
    match = RELEASE_TAG.fullmatch(tag)
    if not match:
        raise UpdateError("版本号必须使用 vX.Y.Z 格式")
    return tuple(int(part) for part in match.groups())


def latest_release(tags: list[str]) -> str | None:
    releases = [(version_tuple(tag), tag) for tag in tags if RELEASE_TAG.fullmatch(tag)]
    return max(releases, default=(None, None))[1]


def parse_env_file(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    if not path.exists():
        return values
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        key = key.removeprefix("export ").strip()
        value = value.strip()
        if len(value) >= 2 and value[0] == value[-1] and value[0] in {'"', "'"}:
            value = value[1:-1]
        values[key] = value
    return values


def manifest_version(path: Path) -> str:
    in_package = False
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if line.startswith("["):
            in_package = line == "[package]"
        elif in_package:
            match = re.fullmatch(r'version\s*=\s*"([^"]+)"', line)
            if match:
                return match.group(1)
    raise UpdateError(f"无法读取版本文件：{path}")


def verify_release_manifests(source: Path, tag: str) -> None:
    expected = tag.removeprefix("v")
    backend = manifest_version(source / "backend" / "Cargo.toml")
    frontend = json.loads((source / "frontend" / "package.json").read_text(encoding="utf-8"))["version"]
    lock = json.loads((source / "frontend" / "package-lock.json").read_text(encoding="utf-8"))
    versions = {backend, frontend, lock["version"], lock["packages"][""]["version"]}
    if versions != {expected}:
        raise UpdateError(f"发布标签 {tag} 与清单版本不一致")


def prune_release_backups(backups: Path, keep: Path) -> list[str]:
    if not backups.is_dir() or not keep.is_dir() or keep.is_symlink():
        raise UpdateError("回滚备份目录无效，已停止自动清理")
    backup_root = backups.resolve()
    if keep.parent.resolve() != backup_root or not ROLLBACK_BACKUP.fullmatch(keep.name):
        raise UpdateError("最新回滚备份不在受控目录中，已停止自动清理")

    removed: list[str] = []
    for candidate in sorted(backups.iterdir(), key=lambda path: path.name):
        if candidate == keep or candidate.is_symlink() or not candidate.is_dir():
            continue
        if not ROLLBACK_BACKUP.fullmatch(candidate.name):
            continue
        if candidate.parent.resolve() != backup_root:
            raise UpdateError("发现越界回滚备份目录，已停止自动清理")
        shutil.rmtree(candidate)
        removed.append(candidate.name)
    return removed


def atomic_json(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{uuid.uuid4().hex}.tmp")
    temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    os.chmod(temporary, 0o640)
    os.replace(temporary, path)


@contextlib.contextmanager
def process_lock(path: Path) -> Iterator[None]:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a+", encoding="utf-8") as handle:
        if fcntl is not None:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX)
        yield


class Updater:
    def __init__(self, root: Path | None = None) -> None:
        self.root = root or Path(os.environ.get("CREWDOCK_ROOT", "/opt/crewdock"))
        self.source = self.root / "source"
        self.env_file = self.root / ".env"
        self.update_root = self.root / "update"
        self.control = self.update_root / "control"
        self.releases = self.root / "releases"
        self.backups = self.root / "backups"
        self.logs = self.update_root / "logs"
        self.docker_config = self.update_root / "docker-config"
        self.status_path = self.control / "status.json"
        self.request_path = self.control / "request.json"
        self.lock_path = self.control / "runner.lock"
        self.log_path: Path | None = None

    def ensure_layout(self) -> None:
        for path in (
            self.control,
            self.releases,
            self.backups,
            self.logs,
            self.docker_config,
        ):
            path.mkdir(parents=True, exist_ok=True)

    def current_version(self) -> str:
        return f"v{manifest_version(self.source / 'backend' / 'Cargo.toml')}"

    def read_status(self) -> dict[str, Any]:
        if not self.status_path.exists():
            return {}
        try:
            return json.loads(self.status_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise UpdateError(f"更新状态文件损坏：{error}") from error

    def update_status(self, **changes: Any) -> dict[str, Any]:
        status = self.read_status()
        status.update({"schema_version": 1, **changes, "updated_at": utc_now()})
        atomic_json(self.status_path, status)
        return status

    def initialize(self) -> None:
        self.ensure_layout()
        self.update_status(
            state="idle",
            operation=None,
            current_version=self.current_version(),
            latest_version=None,
            target_version=None,
            update_available=False,
            message="更新服务已就绪，等待首次检查",
            error=None,
            progress=0,
            checked_at=None,
            started_at=None,
            finished_at=None,
        )

    def begin_log(self, operation: str) -> None:
        stamp = datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S")
        self.log_path = self.logs / f"{stamp}-{operation}.log"
        self.log_path.touch(mode=0o600)

    def command(
        self,
        arguments: list[str],
        *,
        cwd: Path | None = None,
        environment: dict[str, str] | None = None,
        capture: bool = False,
    ) -> str:
        env = os.environ.copy()
        env["DOCKER_CONFIG"] = str(self.docker_config)
        if environment:
            env.update(environment)
        if capture:
            result = subprocess.run(
                arguments,
                cwd=cwd,
                env=env,
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                encoding="utf-8",
                errors="replace",
            )
            if result.returncode:
                raise UpdateError(self.command_error(arguments[0], result.stdout))
            return result.stdout

        if self.log_path is None:
            raise UpdateError("更新日志尚未初始化")
        with self.log_path.open("a", encoding="utf-8") as log:
            log.write(f"\n$ {' '.join(arguments)}\n")
            log.flush()
            result = subprocess.run(
                arguments,
                cwd=cwd,
                env=env,
                check=False,
                stdout=log,
                stderr=subprocess.STDOUT,
            )
        if result.returncode:
            raise UpdateError(f"{arguments[0]} 执行失败，详见主机更新日志")
        return ""

    @staticmethod
    def command_error(command: str, output: str) -> str:
        lines = [line.strip() for line in output.splitlines() if line.strip()]
        detail = lines[-1] if lines else "未返回错误详情"
        return f"{command} 执行失败：{detail[:300]}"

    def git_environment(self) -> dict[str, str]:
        return {"GIT_TERMINAL_PROMPT": "0"}

    def remote_tags(self) -> list[str]:
        output = self.command(
            ["git", "ls-remote", "--tags", "--refs", REPOSITORY_URL],
            environment=self.git_environment(),
            capture=True,
        )
        return [line.rsplit("refs/tags/", 1)[1] for line in output.splitlines() if "refs/tags/" in line]

    def check(self) -> dict[str, Any]:
        self.ensure_layout()
        self.begin_log("check")
        current = self.current_version()
        self.update_status(
            state="checking",
            operation="check",
            current_version=current,
            message="正在读取 GitHub 发布标签",
            error=None,
            progress=25,
            started_at=utc_now(),
            finished_at=None,
        )
        latest = latest_release(self.remote_tags())
        if latest is None:
            raise UpdateError("GitHub 仓库中没有可用的 vX.Y.Z 发布标签")
        available = version_tuple(latest) > version_tuple(current)
        checked_at = utc_now()
        return self.update_status(
            state="available" if available else "up_to_date",
            operation="check",
            current_version=current,
            latest_version=latest,
            target_version=None,
            update_available=available,
            message=f"发现新版本 {latest}" if available else f"当前已是最新版本 {current}",
            error=None,
            progress=100,
            checked_at=checked_at,
            finished_at=checked_at,
        )

    def process_request(self) -> None:
        self.ensure_layout()
        if not self.request_path.exists():
            return
        try:
            request = json.loads(self.request_path.read_text(encoding="utf-8"))
            request_id = str(uuid.UUID(request["request_id"]))
            action = request.get("action")
            if request.get("schema_version") != 1 or action not in {"check", "apply"}:
                raise UpdateError("更新请求格式无效")
            target = request.get("target_version")
            if action == "apply":
                version_tuple(target)
            self.update_status(
                state="checking",
                operation=action,
                current_version=self.current_version(),
                target_version=target,
                message="主机已接收更新任务",
                error=None,
                progress=4,
                started_at=utc_now(),
                finished_at=None,
                request_id=request_id,
            )
            self.request_path.unlink()
            if action == "check":
                self.check()
            else:
                self.apply(target, request_id)
        except Exception as error:
            with contextlib.suppress(OSError):
                self.request_path.unlink()
            self.record_failure(error)

    def apply(self, target: str, request_id: str) -> None:
        current = self.current_version()
        if version_tuple(target) <= version_tuple(current):
            raise UpdateError("目标版本必须高于当前版本")
        self.begin_log(f"apply-{target}")
        started_at = utc_now()
        deployed = False
        backup: Path | None = None
        previous_source = self.source.resolve()
        try:
            self.update_status(
                state="checking",
                operation="apply",
                current_version=current,
                target_version=target,
                message="正在确认最新发布标签",
                error=None,
                progress=8,
                started_at=started_at,
                finished_at=None,
                request_id=request_id,
                log_file=self.log_path.name if self.log_path else None,
            )
            latest = latest_release(self.remote_tags())
            if latest != target:
                raise UpdateError(f"目标版本已变化，请重新检查更新（当前最新：{latest or '无'}）")

            self.update_status(state="backing_up", message="正在备份数据库、源码和容器信息", progress=18)
            backup = self.create_backup(current)
            removed_backups = prune_release_backups(self.backups, backup)
            if self.log_path is not None and removed_backups:
                with self.log_path.open("a", encoding="utf-8") as log:
                    log.write(f"已清理旧回滚备份：{', '.join(removed_backups)}\n")
            self.update_status(
                backup_path=str(backup),
                backup_retention=1,
                pruned_backups=len(removed_backups),
            )

            self.update_status(state="downloading", message=f"正在下载并校验 {target}", progress=32)
            release = self.prepare_release(target)
            commit_sha = self.command(["git", "rev-parse", "HEAD"], cwd=release, capture=True).strip()

            self.update_status(state="building", message="正在构建 API 与 Web 镜像", progress=48)
            environment = self.release_environment(target, commit_sha)
            compose_file = release / "deploy" / "docker-compose.yml"
            self.command(
                [
                    "docker",
                    "compose",
                    "-f",
                    str(compose_file),
                    "--env-file",
                    str(self.env_file),
                    "build",
                    "crewdock-api",
                    "crewdock-web",
                ],
                cwd=release,
                environment=environment,
            )

            self.update_status(state="deploying", message="正在切换 crewdock API 与 Web", progress=78)
            self.command(
                [
                    "docker",
                    "compose",
                    "-f",
                    str(compose_file),
                    "--env-file",
                    str(self.env_file),
                    "up",
                    "-d",
                    "--no-deps",
                    "crewdock-api",
                    "crewdock-web",
                ],
                cwd=release,
                environment=environment,
            )
            deployed = True

            self.update_status(state="verifying", message="正在等待新版本健康检查", progress=92)
            self.wait_for_health(target, timeout=150)
            self.promote_source(release, backup)
            finished_at = utc_now()
            self.update_status(
                state="completed",
                operation="apply",
                current_version=target,
                latest_version=target,
                target_version=target,
                update_available=False,
                message=f"{target} 更新完成",
                error=None,
                progress=100,
                checked_at=finished_at,
                finished_at=finished_at,
                backup_path=str(backup),
            )
        except Exception as error:
            if deployed:
                try:
                    self.rollback(previous_source, current)
                    self.update_status(
                        state="rolled_back",
                        message=f"{target} 更新失败，已恢复 {current}",
                        error=self.clean_error(error),
                        progress=100,
                        finished_at=utc_now(),
                        backup_path=str(backup) if backup else None,
                    )
                    return
                except Exception as rollback_error:
                    error = UpdateError(
                        f"更新失败且自动回滚失败：{self.clean_error(error)}；{self.clean_error(rollback_error)}"
                    )
            raise error

    def create_backup(self, current: str) -> Path:
        stamp = datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S")
        backup = self.backups / f"pre-{current}-{stamp}"
        backup.mkdir(mode=0o700, parents=True, exist_ok=False)
        env = parse_env_file(self.env_file)
        user = env.get("POSTGRES_USER", "crewdock")
        database = env.get("POSTGRES_DB", "crewdock")
        if self.log_path is None:
            raise UpdateError("更新日志尚未初始化")
        with (backup / "database.dump").open("wb") as dump, self.log_path.open("a", encoding="utf-8") as log:
            result = subprocess.run(
                ["docker", "exec", "crewdock-postgres", "pg_dump", "-U", user, "-d", database, "-Fc"],
                check=False,
                stdout=dump,
                stderr=log,
            )
        if result.returncode:
            raise UpdateError("PostgreSQL 备份失败")
        self.command(
            ["tar", "--dereference", "-C", str(self.root), "-czf", str(backup / "source.tar.gz"), "source"]
        )
        inspect = self.command(["docker", "inspect", "crewdock-api", "crewdock-web"], capture=True)
        (backup / "containers.json").write_text(inspect, encoding="utf-8")
        metadata = {
            "created_at": utc_now(),
            "version": current,
            "source": str(self.source.resolve()),
            "api_image": self.container_image("crewdock-api"),
            "web_image": self.container_image("crewdock-web"),
        }
        atomic_json(backup / "metadata.json", metadata)
        return backup

    def container_image(self, name: str) -> str:
        return self.command(
            ["docker", "inspect", "--format", "{{.Config.Image}}", name], capture=True
        ).strip()

    def prepare_release(self, target: str) -> Path:
        release = self.releases / target
        if release.exists():
            verify_release_manifests(release, target)
            return release
        staging = self.releases / f".{target}-{uuid.uuid4().hex}.tmp"
        try:
            self.command(
                [
                    "git",
                    "-c",
                    "advice.detachedHead=false",
                    "clone",
                    "--depth",
                    "1",
                    "--branch",
                    target,
                    REPOSITORY_URL,
                    str(staging),
                ],
                environment=self.git_environment(),
            )
            verify_release_manifests(staging, target)
            staging.rename(release)
            return release
        except Exception:
            if staging.parent == self.releases and staging.exists():
                shutil.rmtree(staging)
            raise

    def release_environment(self, target: str, commit_sha: str) -> dict[str, str]:
        environment = parse_env_file(self.env_file)
        environment.update(
            {
                "CREWDOCK_VERSION": target.removeprefix("v"),
                "CREWDOCK_GIT_SHA": commit_sha[:12],
                "CREWDOCK_BUILD_TIME": utc_now(),
            }
        )
        return environment

    def wait_for_health(self, expected: str, timeout: int) -> None:
        env = parse_env_file(self.env_file)
        port = env.get("CREWDOCK_WEB_PORT", "16888")
        deadline = time.monotonic() + timeout
        last_error = "服务尚未响应"
        while time.monotonic() < deadline:
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=4) as response:
                    payload = json.loads(response.read().decode("utf-8"))["data"]
                actual = f"v{payload.get('version', '')}"
                if payload.get("status") == "ok" and actual == expected:
                    return
                last_error = f"健康接口版本为 {actual or '未知'}"
            except Exception as error:
                last_error = str(error)
            time.sleep(3)
        raise UpdateError(f"健康检查超时：{last_error[:240]}")

    def promote_source(self, release: Path, backup: Path) -> None:
        temporary = self.root / f".source-{uuid.uuid4().hex}.tmp"
        temporary.symlink_to(release, target_is_directory=True)
        if self.source.is_symlink():
            os.replace(temporary, self.source)
            return
        if self.source.exists():
            previous = backup / "source-live"
            self.source.rename(previous)
            try:
                os.replace(temporary, self.source)
            except Exception:
                previous.rename(self.source)
                raise
        else:
            os.replace(temporary, self.source)

    def rollback(self, previous_source: Path, previous_version: str) -> None:
        self.update_status(
            state="rolling_back",
            message=f"健康检查失败，正在恢复 {previous_version}",
            progress=96,
        )
        compose_file = previous_source / "deploy" / "docker-compose.yml"
        environment = parse_env_file(self.env_file)
        environment["CREWDOCK_VERSION"] = previous_version.removeprefix("v")
        self.command(
            [
                "docker",
                "compose",
                "-f",
                str(compose_file),
                "--env-file",
                str(self.env_file),
                "up",
                "-d",
                "--no-deps",
                "crewdock-api",
                "crewdock-web",
            ],
            cwd=previous_source,
            environment=environment,
        )
        self.wait_for_health(previous_version, timeout=120)

    def record_failure(self, error: Exception) -> None:
        current = None
        with contextlib.suppress(Exception):
            current = self.current_version()
        self.update_status(
            state="failed",
            current_version=current,
            message="更新任务失败",
            error=self.clean_error(error),
            progress=100,
            finished_at=utc_now(),
        )

    @staticmethod
    def clean_error(error: Exception) -> str:
        return " ".join(str(error).split())[:600] or error.__class__.__name__


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="crewdock host updater")
    action = parser.add_mutually_exclusive_group(required=True)
    action.add_argument("--initialize", action="store_true")
    action.add_argument("--check", action="store_true")
    action.add_argument("--process-request", action="store_true")
    return parser.parse_args()


def main() -> int:
    arguments = parse_arguments()
    updater = Updater()
    updater.ensure_layout()
    with process_lock(updater.lock_path):
        try:
            if arguments.initialize:
                updater.initialize()
            elif arguments.check:
                updater.check()
            else:
                updater.process_request()
        except Exception as error:
            updater.record_failure(error)
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
