"""The app was called Shoal before it became Tellfold: old commands, variables and folders must keep working."""

import sys
from pathlib import Path

import pytest

from tellfold import cli, settings
from tellfold.runtimes.detect import child_env, is_secret_env


@pytest.fixture
def fake_home(tmp_path, monkeypatch):
    monkeypatch.setattr(Path, "home", lambda: tmp_path)
    for name in ("TELLFOLD_HOME", "SHOAL_HOME"):
        monkeypatch.delenv(name, raising=False)
    return tmp_path / "Library" / "Application Support"


def test_old_application_support_folder_moves_to_tellfold(fake_home):
    old_run = fake_home / "Shoal" / "runs" / "20261107-a"
    old_run.mkdir(parents=True)
    (old_run / "config.json").write_text("{}")

    home = settings.tellfold_home()

    assert home == fake_home / "Tellfold"
    assert (home / "runs" / "20261107-a" / "config.json").read_text() == "{}"
    assert not (fake_home / "Shoal").exists()


def test_existing_tellfold_folder_is_never_overwritten(fake_home):
    (fake_home / "Shoal").mkdir(parents=True)
    (fake_home / "Tellfold").mkdir()

    assert settings.tellfold_home() == fake_home / "Tellfold"
    assert (fake_home / "Shoal").is_dir()


def test_old_variable_names_still_work(fake_home, tmp_path, monkeypatch):
    monkeypatch.delenv("TELLFOLD_FAKE_LLM", raising=False)
    monkeypatch.setenv("SHOAL_FAKE_LLM", "1")
    monkeypatch.setenv("SHOAL_HOME", str(tmp_path / "custom"))

    assert cli.fake_mode()
    assert settings.tellfold_home() == tmp_path / "custom"

    monkeypatch.setenv("TELLFOLD_FAKE_LLM", "0")
    assert not cli.fake_mode(), "the new name wins over the old one"


def test_old_key_and_token_names_stay_secret(monkeypatch):
    assert settings.renamed_key_env("SHOAL_KEY_DEEPSEEK") == "TELLFOLD_KEY_DEEPSEEK"
    assert settings.renamed_key_env("TELLFOLD_KEY_DEEPSEEK") == "SHOAL_KEY_DEEPSEEK"
    assert settings.renamed_key_env("OPENAI_API_KEY") == "OPENAI_API_KEY"
    assert is_secret_env("SHOAL_KEY_DEEPSEEK") and is_secret_env("SHOAL_SERVE_TOKEN")

    monkeypatch.setenv("SHOAL_SERVE_TOKEN", "old-token")
    monkeypatch.setenv("SHOAL_KEY_DEEPSEEK", "old-key")
    env = child_env()
    assert "SHOAL_SERVE_TOKEN" not in env and "SHOAL_KEY_DEEPSEEK" not in env


def test_shoal_command_is_a_deprecated_alias(monkeypatch, capsys):
    monkeypatch.setattr(sys, "argv", ["shoal", "--version"])
    with pytest.raises(SystemExit) as exit_info:
        cli.legacy_main()
    out = capsys.readouterr()
    assert exit_info.value.code in (0, None)
    assert "tellfold" in out.out
    assert "`shoal` is now `tellfold`" in out.err
