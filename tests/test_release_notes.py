"""Exercise release ranges against real Git histories without publishing anything."""

from pathlib import Path
import subprocess

import pytest

from scripts.release_notes import release_notes


@pytest.fixture
def history(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    def git(*args):
        return subprocess.check_output(["git", *args], text=True).strip()

    git("init", "-b", "main")
    git("config", "user.name", "Release test")
    git("config", "user.email", "release@example.invalid")
    git("config", "commit.gpgsign", "false")
    git("config", "tag.gpgsign", "false")

    def commit(version, message):
        Path("Cargo.toml").write_text(f'[workspace.package]\nversion = "{version}"\n')
        git("add", "Cargo.toml")
        git("commit", "--allow-empty", "-m", message)

    return git, commit


def test_first_release_includes_all_commits(history):
    git, commit = history
    commit("0.1.0", "Initial implementation")
    commit("0.1.0", "Handle *mute* and @mentions")
    git("tag", "v0.1.0")
    notes = release_notes("v0.1.0", "example/sidekick")
    assert "Initial release" in notes
    assert "Initial implementation" in notes
    assert r"Handle \*mute\* and &#64;mentions" in notes
    assert f"https://github.com/example/sidekick/commit/{git('rev-parse', 'HEAD')}" in notes


def test_release_uses_previous_reachable_version_tag(history):
    git, commit = history
    commit("0.1.0", "Old changes")
    git("tag", "-a", "v0.1.0", "-m", "Version 0.1.0")
    git("checkout", "-b", "unrelated")
    commit("9.0.0", "Unrelated branch work")
    git("tag", "v9.0.0")
    git("checkout", "main")
    commit("0.2.0", "New feature")
    git("tag", "checkpoint")
    git("tag", "v0.2.0")
    notes = release_notes("v0.2.0", "example/sidekick")
    assert "New feature" in notes
    assert "Old changes" not in notes
    assert "Unrelated branch work" not in notes
    assert "/compare/v0.1.0...v0.2.0" in notes


def test_prerelease_and_same_commit_tags(history):
    git, commit = history
    commit("0.2.0-rc.1", "Candidate")
    git("tag", "v0.1.0")
    git("tag", "v0.2.0-rc.1")
    notes = release_notes("v0.2.0-rc.1", "example/sidekick")
    assert "No new commits" in notes
    assert "/compare/v0.1.0...v0.2.0-rc.1" in notes


def test_release_rejects_wrong_version_or_checkout(history):
    git, commit = history
    commit("0.1.0", "Initial")
    git("tag", "v0.1.0")
    with pytest.raises(ValueError, match="does not match"):
        release_notes("v0.2.0", "example/sidekick")
    with pytest.raises(ValueError, match="Release tags"):
        release_notes("version-one", "example/sidekick")
    commit("0.1.0", "After the tag")
    with pytest.raises(ValueError, match="Check out"):
        release_notes("v0.1.0", "example/sidekick")
