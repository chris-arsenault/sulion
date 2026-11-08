#!/usr/bin/env python3
"""Expire local investigation payloads, leaving source and registry records intact."""

import argparse
import fcntl
import json
import logging
import os
from pathlib import Path
import shutil
import subprocess
import time

LOG = logging.getLogger("storage-maintenance")
RETENTION_DAYS = 21
DAY = 86400
ARTIFACT_ROOTS = (".artifacts", "frontend/.artifacts", "frontend/harness/artifacts")
GROUPS = {"harness", "profiles", "captures", "test-logs"}
SOURCE_SUFFIXES = {
    ".md",
    ".mdx",
    ".txt",
    ".py",
    ".ts",
    ".tsx",
    ".js",
    ".mjs",
    ".sh",
    ".rs",
    ".cs",
}
RECORDS = ("ledger.db", "registry.duckdb")

# Read paths only, never process environments. A quiet run can still have an
# old checkpoint open, or name its next output in argv between writes.
PROC_PATHS = r"""
import glob,json,os,sys
paths=set()
procs=['/proc/'+str(int(pid)) for pid in sys.argv[1:]] or glob.glob('/proc/[0-9]*')
for proc in procs:
    try:
        status=open(proc+'/status').read().splitlines()
        uid=next(line.split()[2] for line in status if line.startswith('Uid:'))
        if int(uid)!=os.geteuid(): continue
        cwd=os.readlink(proc+'/cwd')
        paths.add(cwd)
        for arg in open(proc+'/cmdline','rb').read().split(b'\0'):
            value=os.fsdecode(arg).split('=',1)[-1]
            if '/' in value:
                paths.add(os.path.normpath(os.path.join(cwd,value)))
        for fd in glob.glob(proc+'/fd/*'):
            try: paths.add(os.readlink(fd))
            except FileNotFoundError: pass
    except (FileNotFoundError,ProcessLookupError): pass
print(json.dumps([p for p in paths if p.startswith('/')]))
"""


def command(args, **kwargs):
    return subprocess.run(
        args, check=True, text=True, capture_output=True, **kwargs
    ).stdout


def active_paths():
    paths = json.loads(command(["python3", "-c", PROC_PATHS]))
    containers = command(
        [
            "/usr/bin/docker",
            "ps",
            "--filter",
            "name=sulion-devenv-",
            "--format",
            "{{.ID}}",
        ]
    )
    for container in containers.splitlines():
        paths.extend(
            json.loads(
                command(
                    [
                        "/usr/bin/docker",
                        "exec",
                        "--user",
                        str(os.geteuid()),
                        container,
                        "python3",
                        "-c",
                        PROC_PATHS,
                    ]
                )
            )
        )
    return [Path(p.removesuffix(" (deleted)")) for p in paths]


def protected(path, tracked):
    return (
        path in tracked
        or path.is_symlink()
        or path.suffix.lower() in SOURCE_SUFFIXES
        or path.name == "manifest.json"
        or any(path.name.startswith(name) for name in RECORDS)
    )


def tree_info(path, tracked):
    """Newest entry wins for the whole run; links are never traversed."""

    def unreadable(error):
        raise error

    newest = path.lstat().st_mtime
    size = 0
    entries = [path]
    if path.is_dir() and not path.is_symlink():
        for base, dirs, files in os.walk(path, followlinks=False, onerror=unreadable):
            entries.extend(Path(base) / name for name in dirs + files)
    for entry in entries:
        stat = entry.lstat()
        newest = max(newest, stat.st_mtime)
        if not entry.is_dir() and not protected(entry, tracked):
            size += stat.st_blocks * 512
    return newest, size


def remove_payload(path, tracked):
    if protected(path, tracked):
        return
    if path.is_dir():
        for child in path.iterdir():
            remove_payload(child, tracked)
        if not any(path.iterdir()):
            path.rmdir()
    else:
        path.unlink()


def expire(path, tracked, active, cutoff, apply):
    if path.is_symlink() or any(p == path or p.is_relative_to(path) for p in active):
        return 0
    newest, size = tree_info(path, tracked)
    if newest >= cutoff or size == 0:
        return 0
    LOG.info(
        "%s path=%s payload_bytes=%d", "expire" if apply else "candidate", path, size
    )
    if apply:
        remove_payload(path, tracked)
    return size


def monte_carlo(root, repo, tracked, active, cutoff, apply):
    """The project's CLI owns registry/manifest changes, including soft deletion."""
    registry_path = root / "registry.duckdb"
    if not registry_path.is_file() or registry_path.is_symlink():
        raise RuntimeError(f"Monte Carlo registry missing or linked; preserving {root}")
    if (root / ".deleted").is_symlink():
        raise RuntimeError(f"deleted-run directory is a symlink: {root}")
    assemblies = sorted(
        (repo / ".artifacts/bin/MonteCarlo/Release").glob("*/MonteCarlo.dll")
    )
    if not assemblies:
        raise RuntimeError(f"Monte Carlo CLI missing; preserving {root}")
    cli = ["dotnet", str(assemblies[-1])]
    rows = command(cli + ["list", "--artifact-root", str(root)], cwd=repo)
    registry = {
        r[0]: (r[1], Path(r[3]))
        for line in rows.splitlines()
        if len(r := line.split("\t")) == 4
    }
    total = 0
    for path in list(root.iterdir()):
        if not path.is_dir() or path.is_symlink() or path.name == ".deleted":
            continue
        newest, size = tree_info(path, tracked)
        if (
            newest >= cutoff
            or not size
            or any(p == path or p.is_relative_to(path) for p in active)
        ):
            continue
        record = registry.get(path.name)
        if record is None or record[0] not in {"completed", "failed", "deleted"}:
            LOG.warning("preserving unregistered or unfinished run %s", path)
            continue
        if any(p == path or p.is_relative_to(path) for p in tracked):
            LOG.warning("preserving run containing tracked files %s", path)
            continue
        if not apply:
            LOG.info("candidate path=%s payload_bytes=%d", path, size)
            total += size
            continue
        if record[0] != "deleted":
            command(
                cli
                + ["delete-run", "--run-id", path.name, "--artifact-root", str(root)],
                cwd=repo,
            )
            manifest = json.loads(
                command(
                    cli + ["show", "--run-id", path.name, "--artifact-root", str(root)],
                    cwd=repo,
                )
            )
            destination = Path(manifest["artifactPath"])
            if (
                not destination.resolve().is_relative_to(root / ".deleted")
                or destination.is_symlink()
            ):
                raise RuntimeError(f"unexpected deleted run path: {destination}")
            remove_payload(destination, tracked)
            # CLI moves current paths. Legacy registry paths can name a former
            # home; in that case it leaves today's payload where it was.
            if path.exists():
                remove_payload(path, tracked)
        else:
            remove_payload(path, tracked)
        LOG.info("expired registered run=%s payload_bytes=%d", path.name, size)
        total += size
    deleted = root / ".deleted"
    if deleted.is_dir() and not deleted.is_symlink():
        for path in deleted.iterdir():
            if not path.is_dir() or path.is_symlink():
                continue
            total += expire(path, tracked, active, cutoff, apply)
    return total


def clean_root(root, repo, tracked, active, cutoff, apply):
    if root.is_symlink() or not root.is_dir() or root.resolve() != root.absolute():
        return 0
    total = 0
    for path in list(root.iterdir()):
        if path.name == "bin" or protected(path, tracked):
            continue
        if (
            path.name == "monte-carlo"
            and path.is_dir()
            and (
                (path / "registry.duckdb").exists()
                or (repo / ".artifacts/bin/MonteCarlo").is_dir()
            )
        ):
            total += monte_carlo(path, repo, tracked, active, cutoff, apply)
        elif path.name in GROUPS and path.is_dir():
            total += clean_root(path, repo, tracked, active, cutoff, apply)
        else:
            total += expire(path, tracked, active, cutoff, apply)
    return total


def repositories(repos_root, workspaces_root):
    for repo in repos_root.iterdir():
        if repo.is_dir() and not repo.is_symlink() and (repo / ".git").exists():
            yield repo
    if workspaces_root.is_dir():
        for group in workspaces_root.iterdir():
            if not group.is_dir() or group.is_symlink():
                continue
            if (group / ".git").exists():
                yield group
            else:
                for repo in group.iterdir():
                    if (
                        repo.is_dir()
                        and not repo.is_symlink()
                        and (repo / ".git").exists()
                    ):
                        yield repo


def clean_artifacts(repos_root, workspaces_root, apply, active):
    total = 0
    success = True
    cutoff = time.time() - RETENTION_DAYS * DAY
    for repo in repositories(repos_root, workspaces_root):
        tracked = {
            repo / p
            for p in command(["git", "-C", str(repo), "ls-files", "-z"]).split("\0")
            if p
        }
        for relative in ARTIFACT_ROOTS:
            try:
                total += clean_root(
                    repo / relative, repo, tracked, active, cutoff, apply
                )
            except (OSError, RuntimeError, subprocess.CalledProcessError):
                success = False
                LOG.exception("artifact cleanup failed root=%s", repo / relative)
    return total, success


def cache_cleanup(home, weekly):
    commands = [
        ["/usr/bin/docker", "buildx", "prune", "--force", "--filter", "until=168h"],
        [
            "/usr/bin/docker",
            "image",
            "prune",
            "--all",
            "--force",
            "--filter",
            "until=336h",
        ],
    ]
    weekly_stamp = home / ".cache/sulion/storage-maintenance-weekly"
    if (
        weekly
        or not weekly_stamp.exists()
        or time.time() - weekly_stamp.stat().st_mtime >= 7 * DAY
    ):
        commands.extend(
            [["uv", "cache", "prune"], ["python3", "-m", "pip", "cache", "purge"]]
        )
    success = True
    for args in commands:
        try:
            output = command(args)
            LOG.info("cache command=%s result=%s", args, output.strip())
        except (OSError, subprocess.CalledProcessError) as error:
            success = False
            LOG.error(
                "cache command=%s failed=%s stderr=%s",
                args,
                error,
                getattr(error, "stderr", ""),
            )
    if success and len(commands) > 2:
        weekly_stamp.touch()
    return success


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--home", type=Path, default=Path.home())
    parser.add_argument("--repos-root", type=Path)
    parser.add_argument("--workspaces-root", type=Path)
    parser.add_argument(
        "--apply",
        action="store_true",
        help="delete expired payloads; default only lists candidates",
    )
    parser.add_argument("--artifacts-only", action="store_true")
    parser.add_argument(
        "--weekly",
        action="store_true",
        help="also run the weekly package cache cleanup",
    )
    args = parser.parse_args()
    logging.basicConfig(
        level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s"
    )
    state = args.home / ".cache/sulion"
    state.mkdir(parents=True, exist_ok=True)
    with (state / "storage-maintenance.lock").open("a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            LOG.info("another maintenance cycle is running")
            return 0
        before = shutil.disk_usage(args.home).free
        total, success = 0, True
        try:
            active = active_paths()
            total, success = clean_artifacts(
                args.repos_root or args.home / "repos",
                args.workspaces_root or args.home / "workspaces",
                args.apply,
                active,
            )
        except (OSError, RuntimeError, subprocess.CalledProcessError):
            success = False
            LOG.exception(
                "cannot inspect active runs; preserving investigation artifacts"
            )
        if args.apply and not args.artifacts_only:
            success = cache_cleanup(args.home, args.weekly) and success
        after = shutil.disk_usage(args.home).free
        LOG.info(
            "complete apply=%s candidate_payload_bytes=%d free_before=%d free_after=%d recovered_bytes=%d",
            args.apply,
            total,
            before,
            after,
            after - before,
        )
        return 0 if success else 1


if __name__ == "__main__":
    raise SystemExit(main())
