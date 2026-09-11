"""Validate release version consistency before a tagged build."""

from __future__ import annotations

import argparse
import re
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def _read_project_version() -> str:
    with (ROOT / "pyproject.toml").open("rb") as stream:
        project = tomllib.load(stream)["project"]
    return str(project["version"])


def _read_runtime_version() -> str:
    source = (ROOT / "r105" / "__init__.py").read_text(encoding="utf-8")
    match = re.search(r'^__version__\s*=\s*["\']([^"\']+)["\']', source, re.MULTILINE)
    if match is None:
        raise ValueError("r105/__init__.py does not define __version__")
    return match.group(1)


def validate(tag: str | None = None) -> str:
    project_version = _read_project_version()
    runtime_version = _read_runtime_version()
    if project_version != runtime_version:
        raise ValueError(
            f"version mismatch: pyproject.toml={project_version}, "
            f"r105.__version__={runtime_version}"
        )

    changelog = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
    if not re.search(rf"^## \[{re.escape(project_version)}\](?:\s|$)", changelog, re.MULTILINE):
        raise ValueError(f"CHANGELOG.md has no release section for {project_version}")

    if tag is not None:
        tag_version = tag.removeprefix("v")
        if tag_version != project_version:
            raise ValueError(
                f"tag version mismatch: tag={tag_version}, project={project_version}"
            )
    return project_version


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", help="tag to validate, for example v0.7.0")
    args = parser.parse_args()
    version = validate(args.tag)
    print(f"release metadata is consistent for {version}")


if __name__ == "__main__":
    main()
