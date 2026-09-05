#!/usr/bin/env python3
import argparse
import hashlib
import json
import pathlib
import shutil
import subprocess
import tempfile
import tomllib
from datetime import datetime, timezone

ROOT = pathlib.Path(__file__).resolve().parents[1]


def run(*args):
    subprocess.run(args, cwd=ROOT, check=True)


def workspace_version():
    command = ["cargo", "metadata", "--no-deps", "--format-version", "1"]
    try:
        output = subprocess.check_output(command, cwd=ROOT, text=True)
    except (FileNotFoundError, subprocess.CalledProcessError):
        workspace = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
        version = workspace["workspace"]["package"]["version"]
        for manifest in [
            ROOT / "crates" / "cpa-whale-plugin" / "Cargo.toml",
            ROOT / "crates" / "cpa-whale-admin" / "Cargo.toml",
            ROOT / "crates" / "whale-widget-win" / "Cargo.toml",
        ]:
            package = tomllib.loads(manifest.read_text(encoding="utf-8"))["package"]
            if package.get("version", {}).get("workspace") is not True:
                raise RuntimeError(f"{manifest} does not inherit the workspace version")
        return version
    metadata = json.loads(output)
    versions = {
        package["version"]
        for package in metadata["packages"]
        if package["name"] in {"cpa-whale-plugin", "cpa-whale-admin", "whale-widget-win"}
    }
    if len(versions) != 1:
        raise RuntimeError(f"release components do not share one version: {sorted(versions)}")
    return versions.pop()


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(description="Build a reproducible CPA Whale release bundle")
    parser.add_argument("--output", type=pathlib.Path, default=ROOT / "dist")
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()

    version = workspace_version()
    release_dir = args.output.resolve() / f"cpa-whale-v{version}"
    if release_dir.exists():
        shutil.rmtree(release_dir)
    release_dir.mkdir(parents=True)

    with tempfile.TemporaryDirectory(prefix="cpa-whale-release-") as temporary:
        temporary = pathlib.Path(temporary)
        linux = temporary / "linux"
        windows = temporary / "windows"
        if not args.skip_build:
            run(
                "docker", "build", "-f", "build/plugin.Dockerfile", "--target", "export",
                "--output", f"type=local,dest={linux}", ".",
            )
            run(
                "docker", "build", "-f", "build/windows.Dockerfile", "--target", "export",
                "--output", f"type=local,dest={windows}", ".",
            )
        else:
            linux = ROOT / "build-output" / "release-linux"
            windows = ROOT / "build-output" / "release-windows"

        sources = [
            (linux / "cpa-whale-plugin-linux-amd64.so", f"cpa-whale-plugin-v{version}-linux-amd64.so", "plugin", "linux-amd64"),
            (linux / "cpa-whale-admin-linux-amd64", f"cpa-whale-admin-v{version}-linux-amd64", "admin", "linux-amd64"),
            (windows / "cpa-whale-windows-x64.exe", f"cpa-whale-v{version}-windows-x64.exe", "client", "windows-x64"),
        ]
        artifacts = []
        for source, name, component, platform in sources:
            if not source.is_file():
                raise FileNotFoundError(source)
            destination = release_dir / name
            shutil.copy2(source, destination)
            artifacts.append(
                {
                    "file": name,
                    "component": component,
                    "platform": platform,
                    "version": version,
                    "size_bytes": destination.stat().st_size,
                    "sha256": sha256(destination),
                }
            )

    for source in [
        ROOT / "LICENSE",
        ROOT / "THIRD_PARTY_NOTICES.md",
        ROOT / "deploy" / "plugin-config.example.yaml",
        ROOT / "deploy" / "pricing-gpt-5.6.example.yaml",
        ROOT / "deploy" / "pricing-gpt-6-astra.example.yaml",
        ROOT / "deploy" / "docker-compose.fragment.yaml",
    ]:
        if source.is_file():
            shutil.copy2(source, release_dir / source.name)

    manifest = {
        "release": f"cpa-whale-v{version}",
        "version": version,
        "built_at": datetime.now(timezone.utc).isoformat(),
        "artifacts": artifacts,
    }
    (release_dir / "release-manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    (release_dir / "SHA256SUMS").write_text(
        "".join(f'{artifact["sha256"]}  {artifact["file"]}\n' for artifact in artifacts),
        encoding="utf-8",
    )
    print(release_dir)


if __name__ == "__main__":
    main()
