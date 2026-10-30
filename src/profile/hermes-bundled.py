import hashlib
import os
import shutil
import sys
from pathlib import Path

from hermes_cli.plugins import get_bundled_plugins_dir

LOOM = "loom"
SOURCE = ".loom-source"


def overlay(real, loom, root):
    key = hashlib.sha256(f"{real}\0{loom}".encode()).hexdigest()[:16]
    folder = root / key
    folder.mkdir(parents=True, exist_ok=True)
    wanted = {child.name: child for child in real.iterdir()}
    wanted[LOOM] = loom
    for entry in folder.iterdir():
        if entry.name == SOURCE:
            continue
        target = wanted.get(entry.name)
        if entry.is_symlink() and target is not None and Path(os.readlink(entry)) == target:
            continue
        if entry.is_dir() and not entry.is_symlink():
            shutil.rmtree(entry)
        else:
            entry.unlink()
    for name, target in wanted.items():
        try:
            os.symlink(target, folder / name, target_is_directory=target.is_dir())
        except FileExistsError:
            pass
    (folder / SOURCE).write_text(f"{real}\n{loom}\n", encoding="utf-8")
    return folder


def stale(folder):
    try:
        sources = (folder / SOURCE).read_text(encoding="utf-8").splitlines()
    except OSError:
        return False
    return not all(Path(source).exists() for source in sources)


def clean(root, keep):
    for folder in root.iterdir():
        if folder != keep and folder.is_dir() and stale(folder):
            shutil.rmtree(folder)


def main():
    loom, root = Path(sys.argv[2]).resolve(), Path(sys.argv[3])
    if not (loom / "__init__.py").is_file():
        sys.exit(f"loom plugin not found: {loom}")
    folder = overlay(get_bundled_plugins_dir().resolve(), loom, root)
    clean(root, folder)
    print(folder)


main()
