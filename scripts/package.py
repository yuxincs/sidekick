#!/usr/bin/env python3
"""Build native desktop packages from prebuilt Rust binaries (stdlib only)."""

from __future__ import annotations

import argparse
import hashlib
import os
from pathlib import Path
import plistlib
import shutil
import struct
import subprocess
import tarfile
import tempfile

try:
    import tomllib
except ModuleNotFoundError:
    raise SystemExit("Package building requires Python 3.11 or newer; use uv run python scripts/package.py") from None
import zipfile
import zlib

ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / "packaging"
APP_ID = "dev.HostAgent.HostAgent"


def png(size: int, *, trim: bool = False) -> bytes:
    """Render the Sidekick S tray mark at app-icon resolution."""
    rows = bytearray()
    extent, offset = (240, 8) if trim else (256, 0)
    for y in range(size):
        rows.append(0)
        for x in range(size):
            px, py = offset + (x + 0.5) * extent / size, offset + (y + 0.5) * extent / size
            dx, dy = max(56 - px, 0, px - 200), max(56 - py, 0, py - 200)
            inside = 8 <= px <= 248 and 8 <= py <= 248 and dx * dx + dy * dy <= 48 * 48
            mark = (
                (64 <= px < 192 and (56 <= py < 88 or 112 <= py < 144 or 168 <= py < 200))
                or (64 <= px < 96 and 56 <= py < 144)
                or (160 <= px < 192 and 112 <= py < 200)
            )
            rows.extend((255, 255, 255, 255) if inside and mark else (32, 130, 220, 255) if inside else (0, 0, 0, 0))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(rows))
        + chunk(b"IEND", b"")
    )


def stage_mac(binary: Path, destination: Path, version: str) -> Path:
    app = destination / "Sidekick.app"
    executable = app / "Contents/MacOS/sidekick"
    executable.parent.mkdir(parents=True)
    shutil.copy2(binary, executable)
    executable.chmod(0o755)
    resources = app / "Contents/Resources"
    resources.mkdir()
    shutil.copy2(ROOT / "LICENSE", resources / "LICENSE")
    image = png(1024)
    icon_chunk = b"ic10" + struct.pack(">I", len(image) + 8) + image
    (resources / "HostAgent.icns").write_bytes(b"icns" + struct.pack(">I", len(icon_chunk) + 8) + icon_chunk)
    info = {
        "CFBundleName": "Sidekick",
        "CFBundleDisplayName": "Sidekick",
        "CFBundleIdentifier": APP_ID,
        "CFBundleExecutable": "sidekick",
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": version,
        "CFBundleVersion": version,
        "CFBundleIconFile": "HostAgent.icns",
        "LSUIElement": True,
        "LSMinimumSystemVersion": "11.0",
        "NSHighResolutionCapable": True,
        "NSAppleEventsUsageDescription": "Sidekick can request shutdown only when you enable it in the menu bar.",
    }
    (app / "Contents/Info.plist").write_bytes(plistlib.dumps(info))
    return app


def zip_tree(source: Path, output: Path) -> None:
    with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as archive:
        for path in sorted(source.rglob("*")):
            if path.is_file():
                archive.write(path, path.relative_to(source))


def stage_linux(binary: Path, destination: Path) -> Path:
    folder = destination / "host-agent"
    folder.mkdir()
    shutil.copy2(binary, folder / "sidekick")
    (folder / "sidekick").chmod(0o755)
    for source, target in (
        ("install-linux.sh", "install.sh"),
        ("uninstall-linux.sh", "uninstall.sh"),
        ("host-agent.svg", "host-agent.svg"),
    ):
        shutil.copy2(ASSETS / source, folder / target)
        (folder / target).chmod(0o755 if target.endswith(".sh") else 0o644)
    shutil.copy2(ROOT / "LICENSE", folder)
    (folder / "README.txt").write_text(
        "Install: run ./install.sh, then launch Sidekick from the applications menu.\nChoose Start at login from its tray to enable startup. Quit before updating.\nUninstall: run ~/.local/share/host-agent/uninstall.sh (or the matching XDG_DATA_HOME path).\n"
    )
    return folder


def package_mac(args, stage: Path, prefix: str) -> None:
    app = stage_mac(args.binary, stage, args.version)
    executable = app / "Contents/MacOS/sidekick"
    if args.second_binary:
        merged = stage / "universal-binary"
        subprocess.run(
            ["lipo", "-create", str(args.binary), str(args.second_binary), "-output", str(merged)], check=True
        )
        shutil.copy2(merged, executable)
        merged.unlink()
    arches = subprocess.check_output(["lipo", "-archs", str(executable)], text=True).split()
    required = {"arm64", "x86_64"} if args.arch == "universal" else {"arm64" if args.arch == "aarch64" else args.arch}
    if not required.issubset(arches):
        raise RuntimeError(f"Binary architectures {arches} do not match {args.arch}")
    subprocess.run(
        [
            "codesign",
            "--force",
            "--options",
            "runtime",
            "--sign",
            args.sign_identity,
            "--entitlements",
            str(ASSETS / "macos-entitlements.plist"),
            str(app),
        ],
        check=True,
    )
    subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
    subprocess.run(
        ["ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", str(app), str(args.out / f"{prefix}.zip")], check=True
    )
    (stage / "Applications").symlink_to("/Applications", target_is_directory=True)
    subprocess.run(
        [
            "hdiutil",
            "create",
            "-volname",
            "Sidekick",
            "-srcfolder",
            str(stage),
            "-ov",
            "-format",
            "UDZO",
            str(args.out / f"{prefix}.dmg"),
        ],
        check=True,
    )
    if args.notary_profile:
        dmg = str(args.out / f"{prefix}.dmg")
        subprocess.run(
            ["xcrun", "notarytool", "submit", dmg, "--keychain-profile", args.notary_profile, "--wait"], check=True
        )
        subprocess.run(["xcrun", "stapler", "staple", dmg], check=True)


def package_windows(args, stage: Path, prefix: str) -> None:
    shutil.copy2(args.binary, stage / "sidekick.exe")
    shutil.copy2(ROOT / "LICENSE", stage)
    (stage / "README.txt").write_text(
        "Run sidekick.exe from a permanent location, or use the installer.\nConfigure capabilities, pairing, and Start at login from its tray.\nQuit before installing updates.\n"
    )
    zip_tree(stage, args.out / f"{prefix}.zip")
    compiler = shutil.which("makensis")
    if compiler is None:
        candidate = Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")) / "NSIS/makensis.exe"
        compiler = str(candidate) if candidate.is_file() else None
    if compiler is None:
        raise RuntimeError("NSIS is required to build the Windows installer; install NSIS and retry")
    flag = "/D" if os.name == "nt" else "-D"
    subprocess.run(
        [
            compiler,
            f"{flag}VERSION={args.version}",
            f"{flag}BINARY={args.binary}",
            f"{flag}LICENSE_FILE={ROOT / 'LICENSE'}",
            f"{flag}OUTPUT={args.out / (prefix + '-Setup.exe')}",
            str(ASSETS / "windows.nsi"),
        ],
        check=True,
    )


def stage_deb(binary: Path, destination: Path, version: str, arch: str) -> Path:
    deb = destination / "deb-root"
    for folder in (
        "DEBIAN",
        "usr/bin",
        "usr/share/applications",
        "usr/share/icons/hicolor/scalable/apps",
        "usr/share/doc/host-agent",
    ):
        (deb / folder).mkdir(parents=True, exist_ok=True)
    shutil.copy2(binary, deb / "usr/bin/sidekick")
    (deb / "usr/bin/sidekick").chmod(0o755)
    shutil.copy2(ASSETS / "host-agent.svg", deb / "usr/share/icons/hicolor/scalable/apps/host-agent.svg")
    shutil.copy2(ROOT / "LICENSE", deb / "usr/share/doc/host-agent/copyright")
    (deb / "usr/share/applications/host-agent.desktop").write_text(
        "[Desktop Entry]\nType=Application\nName=Sidekick\nComment=Local Home Assistant metrics and actions\nExec=/usr/bin/sidekick\nTryExec=/usr/bin/sidekick\nIcon=host-agent\nTerminal=false\nCategories=Utility;System;\n"
    )
    architecture = {"x86_64": "amd64", "aarch64": "arm64"}[arch]
    (deb / "DEBIAN/control").write_text(
        f"Package: host-agent\nVersion: {version}\nArchitecture: {architecture}\nMaintainer: Sidekick contributors\nSection: utils\nPriority: optional\nDepends: libc6 (>= 2.35), libgcc-s1\nRecommends: pulseaudio-utils | wireplumber, pulseaudio-utils | pipewire-bin, libegl1, libgl1, dbus-user-session, libx11-6, libxcb1, libxkbcommon0, libwayland-client0\nDescription: Local Home Assistant desktop agent\n Pushes host metrics and exposes locally permitted actions over WebSockets.\n Configure capabilities and login startup from the desktop tray.\n"
    )
    return deb


def validate_binary(binary: Path, platform: str, arch: str) -> None:
    if platform == "macos":
        return  # lipo validates the actual Mach-O slices before signing.
    with binary.open("rb") as executable:
        header = executable.read(64)
        if platform == "windows":
            if len(header) < 64 or header[:2] != b"MZ":
                raise ValueError("Expected a Windows PE executable")
            executable.seek(struct.unpack("<I", header[60:64])[0])
            pe = executable.read(6)
            expected = 0x8664 if arch == "x86_64" else 0xAA64
            if len(pe) != 6 or pe[:4] != b"PE\0\0" or struct.unpack("<H", pe[4:6])[0] != expected:
                raise ValueError("Windows executable architecture does not match package")
        elif (
            len(header) < 20
            or header[:6] != b"\x7fELF\x02\x01"
            or struct.unpack("<H", header[18:20])[0] != {"x86_64": 62, "aarch64": 183}[arch]
        ):
            raise ValueError("Expected a 64-bit Linux ELF executable for the selected architecture")


def package_integration(out: Path, version: str) -> Path:
    """Create the standalone Home Assistant integration archive."""
    integration = out / f"Sidekick-{version}-home-assistant.zip"
    with zipfile.ZipFile(integration, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.write(ROOT / "LICENSE", "custom_components/sidekick/LICENSE")
        for path in sorted((ROOT / "custom_components/sidekick").rglob("*")):
            is_brand = path.parent == ROOT / "custom_components/sidekick/brand" and path.suffix == ".png"
            if path.is_file() and (path.suffix in (".py", ".json") or is_brand) and "__pycache__" not in path.parts:
                archive.write(path, path.relative_to(ROOT))
    return integration


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=["macos", "windows", "linux"], required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--second-binary", type=Path)
    parser.add_argument("--arch", choices=["x86_64", "aarch64", "universal"], required=True)
    parser.add_argument("--out", type=Path, default=ROOT / "dist")
    parser.add_argument(
        "--version", default=tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    )
    parser.add_argument("--sign-identity", default="-")
    parser.add_argument("--notary-profile")
    args = parser.parse_args()
    if args.arch == "universal" and args.platform != "macos":
        parser.error("Universal packages are only supported on macOS")
    args.binary = args.binary.resolve(strict=True)
    validate_binary(args.binary, args.platform, args.arch)
    if args.notary_profile and args.sign_identity == "-":
        parser.error("Notarization requires a Developer ID signing identity")
    args.out = args.out.resolve()
    args.out.mkdir(parents=True, exist_ok=True)
    if args.second_binary:
        args.second_binary = args.second_binary.resolve(strict=True)
    prefix = f"Sidekick-{args.version}-{args.platform}-{args.arch}"
    with tempfile.TemporaryDirectory(prefix="host-agent-package-") as temporary:
        stage = Path(temporary)
        if args.platform == "macos":
            package_mac(args, stage, prefix)
        elif args.platform == "windows":
            package_windows(args, stage, prefix)
        else:
            folder = stage_linux(args.binary, stage)
            with tarfile.open(args.out / f"{prefix}.tar.gz", "w:gz") as archive:
                archive.add(folder, arcname="host-agent")
            deb = stage_deb(args.binary, stage, args.version, args.arch)
            subprocess.run(
                ["dpkg-deb", "--root-owner-group", "--build", str(deb), str(args.out / f"{prefix}.deb")], check=True
            )
    integration = package_integration(args.out, args.version)
    files = sorted(args.out.glob(f"{prefix}.*")) + sorted(args.out.glob(f"{prefix}-Setup.exe")) + [integration]
    manifest = args.out / f"{prefix}-SHA256SUMS.txt"
    manifest.write_text("".join(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n" for path in files))
    print(f"Packages and checksums: {args.out}")


if __name__ == "__main__":
    main()
