#!/usr/bin/env python3
"""Verify exact imports and reconstruct the ordinary HTTP/1 dependency patch."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import tarfile

ROOT = Path(__file__).resolve().parents[3]
PROVENANCE = ROOT / "docs/migration/provenance"


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def inventory(path):
    return {str(p.relative_to(path)): sha256(p) for p in sorted(path.rglob("*")) if p.is_file()}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--archive-cache", type=Path, help="Directory containing the exact official .crate archives")
    args = parser.parse_args()
    receipt = json.loads((PROVENANCE / "ordinary-h1-patch.json").read_text())
    sealed = json.loads((PROVENANCE / "ordinary-transport-official-sources.json").read_text())
    sources = {f"{source['name']}-{source['version']}": source for source in sealed["sources"]}
    with tempfile.TemporaryDirectory(prefix="ordinary-h1-reconstruct-") as temporary:
        reconstructed = Path(temporary)
        for package in receipt["packages"]:
            name = package["package"]
            source = sources[name]
            original = ROOT / package["original_path"]
            assert inventory(original) == source["file_sha256"], f"official original differs: {name}"
            assert inventory(original) == package["original_file_sha256"]
            if args.archive_cache:
                archive = args.archive_cache / f"{name}.crate"
                assert sha256(archive) == source["archive_sha256"], f"archive checksum differs: {name}"
                with tarfile.open(archive) as tar:
                    actual = {}
                    for member in tar.getmembers():
                        if member.isfile():
                            relative = Path(member.name)
                            assert relative.parts[0] == name and ".." not in relative.parts
                            actual[str(Path(*relative.parts[1:]))] = hashlib.sha256(tar.extractfile(member).read()).hexdigest()
                    assert actual == inventory(original), f"archive payload differs: {name}"
            shutil.copytree(original, reconstructed / name)
        for patch in receipt["ordered_patches"]:
            path = ROOT / patch["path"]
            assert sha256(path) == patch["sha256"], f"patch checksum differs: {path}"
            subprocess.run(["patch", "--batch", "--forward", "--fuzz=0", "-p1", "-i", str(path)], cwd=reconstructed, check=True)
        for package in receipt["packages"]:
            name = package["package"]
            expected = package["patched_file_sha256"]
            assert inventory(reconstructed / name) == expected, f"reconstruction differs: {name}"
            assert inventory(ROOT / package["patched_path"]) == expected, f"checked-in patch differs: {name}"
            for license_file in package["license_files"]:
                assert expected[license_file] == package["original_file_sha256"][license_file]
        print("Verified exact official originals, unchanged licenses, ordered zero-fuzz reconstruction, and patched inventories")


if __name__ == "__main__":
    main()
