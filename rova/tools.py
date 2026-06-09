"""Local tool executor — runs tools on the client side."""

from __future__ import annotations

import ast
import datetime
import html.parser
import json
import operator
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

import httpx


def execute_tool_call(
    call: dict[str, Any],
    workspace_dir: Path,
) -> dict[str, Any]:
    """Execute a single tool call locally and return a tool result message."""
    function = call.get("function") or {}
    name = function.get("name")
    raw_arguments = function.get("arguments") or "{}"
    arguments = (
        json.loads(raw_arguments) if isinstance(raw_arguments, str) else raw_arguments
    )

    if name == "execute_python":
        result = execute_python(arguments, workspace_dir)
    elif name == "write_file":
        result = write_file(arguments, workspace_dir)
    elif name == "read_file":
        result = read_file(arguments, workspace_dir)
    elif name == "list_files":
        result = list_files(arguments, workspace_dir)
    elif name == "web_search":
        result = web_search(arguments)
    elif name == "web_fetch":
        result = web_fetch(arguments)
    elif name == "get_time":
        result = get_time()
    elif name == "calculate":
        result = calculate(arguments)
    elif name == "system_info":
        result = system_info()
    else:
        result = f"unknown tool: {name}"

    return {
        "role": "tool",
        "tool_call_id": call.get("id", ""),
        "name": name,
        "content": result if isinstance(result, str) else json.dumps(result, sort_keys=True),
    }


# -- Python execution (sandboxed) ---------------------------------------

def _sandbox_preexec() -> None:
    """Set resource limits for sandboxed Python execution (Unix only)."""
    if sys.platform == "win32":
        return  # resource module is Unix-only; skip sandbox limits on Windows
    import resource

    # 256 MB memory limit
    mem_bytes = 256 * 1024 * 1024
    resource.setrlimit(resource.RLIMIT_AS, (mem_bytes, mem_bytes))
    # 25s CPU time limit
    resource.setrlimit(resource.RLIMIT_CPU, (25, 25))
    # No child processes
    resource.setrlimit(resource.RLIMIT_NPROC, (0, 0))
    # Limit file size to 50MB
    resource.setrlimit(resource.RLIMIT_FSIZE, (50 * 1024 * 1024, 50 * 1024 * 1024))


def execute_python(arguments: dict[str, Any], workspace_dir: Path) -> str:
    """Execute Python code in a sandboxed subprocess.

    Uses resource limits (Unix) and stripped environment for basic isolation.
    NOTE: For production multi-tenant use, replace this with container-based
    isolation (Docker, WASM runtime, or gVisor) — the current approach runs
    under the host UID and can read host files like ~/.aws/credentials.
    """
    code = arguments.get("code", "")
    tmpdir = tempfile.mkdtemp(prefix="rova_sandbox_")
    try:
        # Use the same Python interpreter that runs Rova (handles venv, Windows, etc.)
        kwargs: dict[str, Any] = {
            "capture_output": True,
            "text": True,
            "timeout": 30,
            "cwd": tmpdir,
            "env": {
                "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
                "HOME": tmpdir,
                "TMPDIR": tmpdir,
                "PYTHONPATH": "",
            },
        }
        # preexec_fn is Unix-only
        if sys.platform != "win32":
            kwargs["preexec_fn"] = _sandbox_preexec

        result = subprocess.run([sys.executable, "-c", code], **kwargs)
        return result.stdout if result.returncode == 0 else result.stderr
    except subprocess.TimeoutExpired:
        # Python 3.11+ sends SIGKILL to the child on TimeoutExpired.
        # On older Pythons the child may linger as a zombie; if that
        # matters, switch to Popen + process_group + os.killpg.
        return "error: execution timed out (30s)"
    except Exception as e:
        return str(e)
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)


# -- File operations ----------------------------------------------------

def write_file(arguments: dict[str, Any], workspace_dir: Path) -> str:
    path = arguments.get("path", "")
    content = arguments.get("content", "")
    try:
        file_path = _resolve_path(path, workspace_dir)
        file_path.parent.mkdir(parents=True, exist_ok=True)
        file_path.write_text(content, encoding="utf-8")
        return f"wrote {len(content)} bytes to {file_path}"
    except Exception as e:
        return str(e)


def read_file(arguments: dict[str, Any], workspace_dir: Path) -> str:
    path = arguments.get("path", "")
    try:
        file_path = _resolve_path(path, workspace_dir)
        return file_path.read_text(encoding="utf-8", errors="replace")
    except Exception as e:
        return str(e)


def list_files(arguments: dict[str, Any], workspace_dir: Path) -> str:
    path = arguments.get("path", ".")
    try:
        target = _resolve_path(path, workspace_dir)
        if not target.exists():
            return f"path not found: {target}"
        entries = []
        for f in sorted(target.iterdir()):
            kind = "dir" if f.is_dir() else "file"
            size = f.stat().st_size
            entries.append(f"{f.name} ({kind}, {size} bytes)")
        return "\n".join(entries) if entries else "empty directory"
    except Exception as e:
        return str(e)


def _resolve_path(path: str, workspace_dir: Path) -> Path:
    """Resolve a path safely within the workspace directory.

    Absolute paths are treated as relative to the workspace root to prevent
    path traversal attacks. Relative paths stay within the workspace.

    Raises PermissionError if the resolved path escapes the workspace.
    """
    p = Path(path)
    workspace_resolved = workspace_dir.resolve()

    if p.is_absolute():
        # Strip the root anchor and force it relative to the workspace
        try:
            resolved = (workspace_resolved / p.relative_to(p.anchor)).resolve()
        except ValueError:
            resolved = (workspace_resolved / p.name).resolve()
    else:
        resolved = (workspace_resolved / p).resolve()

    # Strict containment check — no path may escape the workspace
    try:
        resolved.relative_to(workspace_resolved)
    except ValueError:
        raise PermissionError(
            f"Access denied: '{path}' resolves outside the workspace ({workspace_resolved})"
        )

    return resolved


# -- Web tools ----------------------------------------------------------

def web_search(arguments: dict[str, Any]) -> str:
    """Search the web using DuckDuckGo HTML (no API key required)."""
    query = arguments.get("query", "")
    if not query:
        return "error: query is required"

    try:
        response = httpx.get(
            "https://html.duckduckgo.com/html/",
            params={"q": query},
            timeout=15.0,
            headers={"User-Agent": "rova/0.2.0"},
            follow_redirects=True,
        )
        response.raise_for_status()
        results = _parse_ddg_results(response.text)
        if not results:
            return f"no results found for: {query}"
        return json.dumps(results, indent=2, ensure_ascii=False)
    except httpx.HTTPError as e:
        return f"search error: {e}"
    except Exception as e:
        return f"search error: {e}"


def web_fetch(arguments: dict[str, Any]) -> str:
    """Fetch a URL and return its text content (HTML tags stripped)."""
    url = arguments.get("url", "")
    max_length = arguments.get("max_length", 8000)
    if not url:
        return "error: url is required"

    try:
        response = httpx.get(
            url,
            timeout=15.0,
            headers={"User-Agent": "rova/0.2.0"},
            follow_redirects=True,
        )
        response.raise_for_status()
        text = _strip_html(response.text)
        if len(text) > max_length:
            text = text[:max_length] + f"\n... (truncated, original: {len(text)} chars)"
        return text
    except httpx.HTTPError as e:
        return f"fetch error: {e}"
    except Exception as e:
        return f"fetch error: {e}"


def _parse_ddg_results(html: str) -> list[dict[str, str]]:
    """Extract search results from DuckDuckGo HTML response."""
    results: list[dict[str, str]] = []
    # DDG HTML results are in <a class="result__a"> for titles
    # and <a class="result__snippet"> for snippets
    title_pattern = re.compile(
        r'<a[^>]*class="result__a"[^>]*>(.*?)</a>', re.DOTALL | re.IGNORECASE
    )
    snippet_pattern = re.compile(
        r'<a[^>]*class="result__snippet"[^>]*>(.*?)</a>', re.DOTALL | re.IGNORECASE
    )
    url_pattern = re.compile(
        r'<a[^>]*class="result__url"[^>]*>(.*?)</a>', re.DOTALL | re.IGNORECASE
    )

    titles = title_pattern.findall(html)
    snippets = snippet_pattern.findall(html)
    urls = url_pattern.findall(html)

    for i, title in enumerate(titles[:10]):
        results.append({
            "title": _clean_html(title),
            "url": _clean_html(urls[i]) if i < len(urls) else "",
            "snippet": _clean_html(snippets[i]) if i < len(snippets) else "",
        })

    return results


class _HTMLStripper(html.parser.HTMLParser):
    """Structural HTML stripper that extracts readable text.

    Uses stdlib HTMLParser for robust parsing. Skips <script>, <style>,
    and <noscript> content. Emits newlines for block-level elements.
    """

    BLOCK_TAGS = {
        "div", "p", "br", "li", "h1", "h2", "h3", "h4", "h5", "h6",
        "tr", "article", "section", "header", "footer", "nav", "main",
        "ul", "ol", "dl", "table", "blockquote", "pre", "hr", "form",
        "fieldset", "figure", "figcaption", "details", "summary",
    }
    SKIP_TAGS = {"script", "style", "noscript", "head", "meta", "link", "title"}

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self._parts: list[str] = []
        self._skip_depth = 0

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag in self.SKIP_TAGS:
            self._skip_depth += 1

    def handle_endtag(self, tag: str) -> None:
        if tag in self.SKIP_TAGS and self._skip_depth > 0:
            self._skip_depth -= 1
        if tag in self.BLOCK_TAGS:
            self._parts.append("\n")

    def handle_data(self, data: str) -> None:
        if self._skip_depth > 0:
            return
        text = data.strip()
        if text:
            self._parts.append(text)
            self._parts.append(" ")

    def get_text(self) -> str:
        raw = "".join(self._parts)
        # Collapse whitespace
        raw = re.sub(r'[ \t]+', ' ', raw)
        raw = re.sub(r'\n\s*\n', '\n\n', raw)
        return raw.strip()


def _strip_html(html: str) -> str:
    """Strip HTML tags and return plain text using a structural parser."""
    stripper = _HTMLStripper()
    try:
        stripper.feed(html)
        stripper.close()
        return stripper.get_text()
    except Exception:
        # Fallback to regex for malformed input
        text = re.sub(r'<script[^>]*>.*?</script>', '', html, flags=re.DOTALL | re.IGNORECASE)
        text = re.sub(r'<style[^>]*>.*?</style>', '', text, flags=re.DOTALL | re.IGNORECASE)
        text = re.sub(r'<[^>]+>', ' ', text)
        text = re.sub(r'[ \t]+', ' ', text)
        text = re.sub(r'\n\s*\n', '\n\n', text)
        return text.strip()


def _clean_html(text: str) -> str:
    """Remove HTML tags from a short snippet."""
    return _strip_html(text)


# -- Utility tools ------------------------------------------------------

# Allowed operators and functions for safe calculate()
_SAFE_OPS: dict[type, Any] = {
    ast.Add: operator.add,
    ast.Sub: operator.sub,
    ast.Mult: operator.mul,
    ast.Div: operator.truediv,
    ast.FloorDiv: operator.floordiv,
    ast.Mod: operator.mod,
    ast.Pow: operator.pow,
    ast.USub: operator.neg,
    ast.UAdd: operator.pos,
}


def _safe_eval(node: ast.AST) -> Any:
    """Recursively evaluate a safe AST expression (no builtins, no calls)."""
    if isinstance(node, ast.Constant):
        return node.value
    if isinstance(node, ast.UnaryOp):
        op = _SAFE_OPS.get(type(node.op))
        if op is None:
            raise ValueError(f"unsafe operator: {type(node.op).__name__}")
        return op(_safe_eval(node.operand))
    if isinstance(node, ast.BinOp):
        op = _SAFE_OPS.get(type(node.op))
        if op is None:
            raise ValueError(f"unsafe operator: {type(node.op).__name__}")
        return op(_safe_eval(node.left), _safe_eval(node.right))
    raise ValueError(f"unsafe expression: {type(node).__name__}")


def get_time() -> str:
    """Return current system time in ISO format."""
    return datetime.datetime.now().isoformat()


def calculate(arguments: dict[str, Any]) -> str:
    """Safely evaluate a mathematical expression. Only arithmetic allowed."""
    expression = arguments.get("expression", "")
    if not expression:
        return "error: expression is required"
    try:
        tree = ast.parse(expression.strip(), mode="eval")
        result = _safe_eval(tree.body)
        return str(result)
    except (SyntaxError, ValueError, ZeroDivisionError) as exc:
        return f"calculate error: {exc}"


def system_info() -> str:
    """Return basic OS and hardware information as JSON."""
    import socket
    info = {
        "platform": platform.platform(),
        "python_version": platform.python_version(),
        "cpu_count": os.cpu_count(),
        "hostname": socket.gethostname(),
        "machine": platform.machine(),
    }
    return json.dumps(info, indent=2, sort_keys=True)


# -- Tool definitions (JSON Schema for the LLM) -------------------------

TOOL_DEFINITIONS: list[dict[str, Any]] = [
    {
        "type": "function",
        "function": {
            "name": "execute_python",
            "description": "Execute Python code and return stdout or stderr.",
            "parameters": {
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "Python source code to execute.",
                    },
                },
                "required": ["code"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "write_file",
            "description": "Write content to a file in the workspace.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path (relative to workspace or absolute).",
                    },
                    "content": {
                        "type": "string",
                        "description": "File content to write.",
                    },
                },
                "required": ["path", "content"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "read_file",
            "description": "Read the contents of a file.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path to read.",
                    },
                },
                "required": ["path"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "list_files",
            "description": "List files in a directory.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Directory path to list (default: workspace root).",
                    },
                },
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "web_search",
            "description": "Search the web and return results with titles, URLs, and snippets.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query string.",
                    },
                },
                "required": ["query"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "web_fetch",
            "description": "Fetch a URL and return its text content (HTML tags removed).",
            "parameters": {
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "URL to fetch.",
                    },
                    "max_length": {
                        "type": "integer",
                        "description": "Maximum characters to return (default: 8000).",
                    },
                },
                "required": ["url"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "get_time",
            "description": "Return the current system time in ISO 8601 format.",
            "parameters": {
                "type": "object",
                "properties": {},
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "calculate",
            "description": "Safely evaluate a mathematical expression (+, -, *, /, **, %, parentheses).",
            "parameters": {
                "type": "object",
                "properties": {
                    "expression": {
                        "type": "string",
                        "description": "Mathematical expression to evaluate, e.g. '2 + 3 * 4'.",
                    },
                },
                "required": ["expression"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "system_info",
            "description": "Return basic OS and hardware information (platform, CPU count, hostname).",
            "parameters": {
                "type": "object",
                "properties": {},
            },
        },
    },
]
