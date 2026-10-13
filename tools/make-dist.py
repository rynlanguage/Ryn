#!/usr/bin/env python3
"""Builds the release archives with the native compiler, without Cargo or a Rust compiler.

Usage: tools/make-dist.py /absolute/path/to/rync VERSION OUTPUT_DIRECTORY [--windows] [--linux]

`rync` is the bootstrap fixpoint (the `rync-stage2` that `tools/bootstrap-native.sh` writes). From the sources in this
checkout it builds the Linux `ryn` and `rync` programs, and with `--windows` the Windows `ryn.exe` and `rync.exe`
(cross-compiled; no Windows machine is needed), and writes:

    ryn-VERSION-linux-x86_64.tar.gz      ryn, rync, stdlib, the sources and the bootstrap script (+ .sha256)
    ryn-VERSION-windows-x86_64.zip       ryn.exe, rync.exe, stdlib (+ .sha256)

`ryn` finds `rync` and `stdlib/std/src` next to itself, so each archive is one directory that can be put anywhere.
"""
import hashlib
import io
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def run(command):
    done = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if done.returncode != 0:
        sys.exit(f"{' '.join(map(str, command))}\n{done.stdout}{done.stderr}")


def compile_program(rync, source, output, windows):
    command = [rync, "build", "--stdlib", (ROOT / "stdlib/std/src").as_posix(), "-o", Path(output).as_posix(), (ROOT / source).as_posix()]
    if windows:
        command[2:2] = ["--target", "windows"]
    run(command)


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def copy_tree(source, destination):
    shutil.copytree(source, destination, ignore=shutil.ignore_patterns("*.pyc", "__pycache__"))


def write_checksum(archive):
    Path(str(archive) + ".sha256").write_text(f"{digest(archive)}  {archive.name}\n")


def linux_archive(rync, version, output, work):
    name = f"ryn-{version}-linux-x86_64"
    stage = work / name
    (stage / "tools").mkdir(parents=True)
    (stage / "docs").mkdir()
    (stage / "bootstrap").mkdir()
    (stage / "selfhost").mkdir()
    (stage / "stdlib/std").mkdir(parents=True)
    compile_program(rync, "selfhost/src/ryn.ryn", stage / "ryn", False)
    compile_program(rync, "selfhost/src/rync.ryn", stage / "rync", False)
    shutil.copy(ROOT / "bootstrap/rync-seed", stage / "bootstrap/rync-seed")
    shutil.copy(ROOT / "bootstrap/rync-seed.sha256", stage / "bootstrap/rync-seed.sha256")
    shutil.copy(ROOT / "README.md", stage / "README.md")
    shutil.copy(ROOT / "tools/bootstrap-native.sh", stage / "tools/")
    copy_tree(ROOT / "selfhost/src", stage / "selfhost/src")
    copy_tree(ROOT / "stdlib/std/src", stage / "stdlib/std/src")
    for doc in ("native-compiler.md", "native-ownership-validation.md", "native-float-validation.md"):
        shutil.copy(ROOT / "docs" / doc, stage / "docs" / doc)
    for program in ("ryn", "rync"):
        os.chmod(stage / program, 0o755)
    archive = output / f"{name}.tar.gz"
    with tarfile.open(archive, "w:gz") as tar:
        tar.add(stage, arcname=name, filter=lambda info: _plain(info))
    write_checksum(archive)
    return archive


def _plain(info):
    info.uid = info.gid = 0
    info.uname = info.gname = ""
    return info


def windows_archive(rync, version, output, work):
    name = f"ryn-{version}-windows-x86_64"
    stage = work / name
    (stage / "stdlib/std").mkdir(parents=True)
    compile_program(rync, "selfhost/src/ryn.ryn", stage / "ryn.exe", True)
    compile_program(rync, "selfhost/src/rync.ryn", stage / "rync.exe", True)
    shutil.copy(ROOT / "README.md", stage / "README.md")
    copy_tree(ROOT / "stdlib/std/src", stage / "stdlib/std/src")
    archive = output / f"{name}.zip"
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as bundle:
        for path in sorted(stage.rglob("*")):
            if path.is_file():
                bundle.write(path, f"{name}/{path.relative_to(stage).as_posix()}")
    write_checksum(archive)
    return archive


def main():
    arguments = [a for a in sys.argv[1:] if not a.startswith("--")]
    flags = {a for a in sys.argv[1:] if a.startswith("--")}
    if len(arguments) != 3 or not flags <= {"--windows", "--linux"}:
        sys.exit(__doc__)
    rync, version, output = arguments
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    both = not flags
    with tempfile.TemporaryDirectory(prefix="ryn-dist-") as directory:
        work = Path(directory)
        if both or "--linux" in flags:
            print(linux_archive(rync, version, output, work))
        if both or "--windows" in flags:
            print(windows_archive(rync, version, output, work))


if __name__ == "__main__":
    main()
