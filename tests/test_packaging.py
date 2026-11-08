"""Packaging and installation checks; never touch the user's startup settings."""

import os
import json
import plistlib
import shlex
import struct
import subprocess
import zipfile

import pytest

from scripts.package import png, stage_mac, stage_linux, stage_deb, zip_tree, ROOT, validate_binary, package_integration


@pytest.fixture
def binary(tmp_path):
    path = tmp_path / "test-agent"
    path.write_text("#!/bin/sh\nexit 0\n")
    path.chmod(0o755)
    return path


def test_mac_bundle_has_gui_metadata_and_an_executable(binary, tmp_path):
    destination = tmp_path / "stage"
    destination.mkdir()
    app = stage_mac(binary, destination, "1.2.3")
    info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
    assert info["LSUIElement"] is True
    assert info["CFBundleExecutable"] == "sidekick"
    assert info["CFBundleIdentifier"] == "dev.HostAgent.HostAgent"
    assert info["CFBundleShortVersionString"] == "1.2.3"
    assert info["NSAppleEventsUsageDescription"]
    executable = app / "Contents/MacOS" / info["CFBundleExecutable"]
    assert executable.read_bytes() == binary.read_bytes()
    assert executable.stat().st_mode & 0o111
    icon = app / "Contents/Resources" / info["CFBundleIconFile"]
    assert icon.read_bytes().startswith(b"icns")
    assert struct.unpack(">I", icon.read_bytes()[4:8])[0] == icon.stat().st_size


def test_icon_is_a_valid_rgba_png():
    image = png(32)
    assert image.startswith(b"\x89PNG\r\n\x1a\n")
    assert struct.unpack(">II", image[16:24]) == (32, 32)
    assert image[24:26] == bytes([8, 6])


def test_zip_preserves_package_layout(binary, tmp_path):
    destination = tmp_path / "stage"
    destination.mkdir()
    stage_mac(binary, destination, "1.2.3")
    archive = tmp_path / "app.zip"
    zip_tree(destination, archive)
    with zipfile.ZipFile(archive) as package:
        assert "Sidekick.app/Contents/MacOS/sidekick" in package.namelist()
        assert all(not name.startswith("/") for name in package.namelist())
        mode = package.getinfo("Sidekick.app/Contents/MacOS/sidekick").external_attr >> 16
        assert mode & 0o111


def desktop_argument(entry):
    line = next(line.removeprefix("Exec=") for line in entry.splitlines() if line.startswith("Exec="))
    # GLib's two parsing stages: desktop string decoding then quoted Exec arguments.
    value = line.replace("\\\\", "\\")
    arguments = shlex.split(value)
    return arguments[0].replace("\\$", "$").replace("\\`", "`").replace("%%", "%")


@pytest.mark.skipif(os.name == "nt", reason="Unix installer")
def test_linux_install_uninstall_handles_special_paths_and_preserves_config(binary, tmp_path):
    stage = tmp_path / "stage"
    stage.mkdir()
    bundle = stage_linux(binary, stage)
    data = tmp_path / "data space $cash% and `tick`"
    config = tmp_path / "config"
    env = {**os.environ, "XDG_DATA_HOME": str(data), "XDG_CONFIG_HOME": str(config)}
    legacy = data / "host-agent/host-agent"
    legacy.parent.mkdir(parents=True)
    legacy.write_text("old executable")
    subprocess.run(["sh", str(bundle / "install.sh")], env=env, check=True, capture_output=True)
    installed = data / "host-agent/sidekick"
    assert installed.read_bytes() == binary.read_bytes()
    desktop = (data / "applications/host-agent.desktop").read_text()
    assert desktop_argument(desktop) == str(installed)
    assert "Terminal=false" in desktop
    assert not (data / "host-agent/host-agent").exists()
    assert not (config / "autostart").exists()  # Startup remains opt-in.
    keep = config / "hostagent/config.json"
    keep.parent.mkdir(parents=True)
    keep.write_text("keep pairing identity")
    startup = config / "autostart/dev.HostAgent.HostAgent.desktop"
    startup.parent.mkdir()
    startup.write_text("test startup registration")
    subprocess.run(["sh", str(data / "host-agent/uninstall.sh")], env=env, check=True, capture_output=True)
    assert not installed.exists()
    assert not startup.exists()
    assert not (data / "applications/host-agent.desktop").exists()
    assert keep.read_text() == "keep pairing identity"


def test_debian_package_has_expected_layout_and_no_automatic_startup(binary, tmp_path):
    root = stage_deb(binary, tmp_path, "1.2.3", "x86_64")
    control = (root / "DEBIAN/control").read_text()
    assert "Architecture: amd64" in control
    assert "Version: 1.2.3" in control
    assert "libvulkan1" in control
    assert "mesa-vulkan-drivers | vulkan-icd" in control
    assert (root / "usr/bin/sidekick").read_bytes() == binary.read_bytes()
    assert (root / "usr/share/applications/host-agent.desktop").is_file()
    assert not (root / "etc/xdg/autostart").exists()


def test_windows_installer_uses_only_per_user_registration():
    installer = (ROOT / "packaging/windows.nsi").read_text()
    assert "RequestExecutionLevel user" in installer
    assert "HKLM" not in installer
    assert (
        'DeleteRegValue HKCU "Software\\Microsoft\\Windows\\CurrentVersion\\Run" "dev.HostAgent.HostAgent"' in installer
    )
    assert 'WriteRegStr HKCU "Software\\Microsoft\\Windows\\CurrentVersion\\Run"' not in installer


@pytest.mark.parametrize("platform", ["windows", "linux"])
def test_wrong_platform_binary_is_rejected(binary, platform):
    with pytest.raises(ValueError):
        validate_binary(binary, platform, "x86_64")


def test_integration_archive_installs_only_the_sidekick_domain(tmp_path):
    archive = package_integration(tmp_path, "test")
    with zipfile.ZipFile(archive) as package:
        names = package.namelist()
        assert all(name.startswith("custom_components/sidekick/") for name in names)
        manifest = json.loads(package.read("custom_components/sidekick/manifest.json"))
        assert manifest["domain"] == "sidekick" and manifest["config_flow"]
        for filename in (
            "__init__.py",
            "config_flow.py",
            "pairing.py",
            "sensor.py",
            "button.py",
            "switch.py",
            "binary_sensor.py",
            "LICENSE",
        ):
            assert f"custom_components/sidekick/{filename}" in names
        for filename, size in (("icon.png", 256), ("icon@2x.png", 512)):
            image = package.read(f"custom_components/sidekick/brand/{filename}")
            assert image.startswith(b"\x89PNG\r\n\x1a\n")
            assert struct.unpack(">II", image[16:24]) == (size, size)
            assert image == (ROOT / "custom_components/sidekick/brand" / filename).read_bytes()
        assert not any("__pycache__" in name for name in names)
