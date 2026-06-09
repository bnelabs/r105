"""Tests for skill loading and parameter substitution."""

from __future__ import annotations

from pathlib import Path

from rova.skills import list_skills, read_skill


class TestListSkills:
    """Tests for list_skills()."""

    def test_empty_dir(self, tmp_path):
        skills = list_skills(tmp_path)
        assert skills == []

    def test_missing_dir(self):
        skills = list_skills(Path("/nonexistent/path"))
        assert skills == []

    def test_lists_md_files(self, tmp_path):
        (tmp_path / "skill-a.md").write_text("skill a")
        (tmp_path / "skill-b.md").write_text("skill b")
        (tmp_path / "not-a-skill.txt").write_text("txt")
        skills = list_skills(tmp_path)
        assert sorted(skills) == ["skill-a", "skill-b"]

    def test_ignores_directories(self, tmp_path):
        (tmp_path / "skill-a.md").write_text("a")
        (tmp_path / "subdir").mkdir()
        skills = list_skills(tmp_path)
        assert skills == ["skill-a"]


class TestReadSkill:
    """Tests for read_skill()."""

    def test_read_existing(self, tmp_path):
        (tmp_path / "test-skill.md").write_text("Be helpful.")
        text = read_skill(tmp_path, "test-skill")
        assert text == "Be helpful."

    def test_read_missing(self, tmp_path):
        text = read_skill(tmp_path, "nonexistent")
        assert text == ""

    def test_path_traversal_blocked(self, tmp_path):
        text = read_skill(tmp_path, "../etc/passwd")
        assert text == ""

    def test_dotfile_blocked(self, tmp_path):
        text = read_skill(tmp_path, ".secret")
        assert text == ""

    def test_backslash_blocked(self, tmp_path):
        text = read_skill(tmp_path, "foo\\bar")
        assert text == ""


class TestSkillParameters:
    """Tests for parameter substitution in skills."""

    def test_simple_substitution(self, tmp_path):
        (tmp_path / "search.md").write_text(
            "Search for {query} and return results."
        )
        text = read_skill(tmp_path, "search", {"query": "Python"})
        assert "Search for Python" in text
        assert "{query}" not in text

    def test_multiple_params(self, tmp_path):
        (tmp_path / "template.md").write_text(
            "Use {language} with {framework}."
        )
        text = read_skill(
            tmp_path,
            "template",
            {"language": "Python", "framework": "FastAPI"},
        )
        assert "Python" in text
        assert "FastAPI" in text
        assert "{" not in text

    def test_no_params_needed(self, tmp_path):
        (tmp_path / "plain.md").write_text("Just plain text.")
        text = read_skill(tmp_path, "plain")
        assert text == "Just plain text."

    def test_none_params(self, tmp_path):
        (tmp_path / "plain.md").write_text("Text with {unused} placeholder.")
        text = read_skill(tmp_path, "plain", None)
        assert "{unused}" in text  # Not substituted

    def test_empty_params(self, tmp_path):
        (tmp_path / "plain.md").write_text("Text with {unused} placeholder.")
        text = read_skill(tmp_path, "plain", {})
        assert "{unused}" in text  # Not substituted

    def test_missing_param_left_unchanged(self, tmp_path):
        (tmp_path / "partial.md").write_text("Hello {name}, welcome.")
        text = read_skill(tmp_path, "partial", {"other": "x"})
        assert "{name}" in text  # Only matching keys are replaced

    def test_partial_match_not_replaced(self, tmp_path):
        """Keys should be exact matches, not substrings."""
        (tmp_path / "x.md").write_text("{key_name}")
        text = read_skill(tmp_path, "x", {"key": "val"})
        assert "{key_name}" in text  # "key" != "key_name"
