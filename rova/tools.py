"""Local tool executor — runs tools on the client side."""

from __future__ import annotations

import ast
import datetime
import html.parser
import ipaddress
import json
import operator
import os
import platform
import re
import socket
import subprocess
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

import httpx

from rova.constants import (
    WEB_FETCH_MAX_CHARS,
    WEB_FETCH_TIMEOUT,
    WEB_SEARCH_MAX_RESULTS,
    WEB_SEARCH_TIMEOUT,
)
from rova.mcp_client import get_mcp_manager
from rova.plugins import get_registry
from rova.sandbox import get_sandbox, profile_for_tool, SandboxProfile

# -- Limits for tool arguments ------------------------------------------

_MAX_CODE_SIZE = 100 * 1024          # 100 KB Python code
_MAX_FILE_CONTENT = 10 * 1024 * 1024 # 10 MB file write
_MAX_FILE_READ = 50 * 1024 * 1024    # 50 MB file read
_MAX_SEARCH_QUERY = 500              # chars

# -- URL validation -----------------------------------------------------

_ALLOWED_URL_SCHEMES = {"http", "https"}

# Private/reserved network blocks (IPv4)
_PRIVATE_NETS = [
    ipaddress.ip_network("10.0.0.0/8"),
    ipaddress.ip_network("127.0.0.0/8"),
    ipaddress.ip_network("169.254.0.0/16"),
    ipaddress.ip_network("172.16.0.0/12"),
    ipaddress.ip_network("192.168.0.0/16"),
    ipaddress.ip_network("0.0.0.0/8"),
]


def _check_ssrf(url_str: str) -> str | None:
    """Return an error string if *url_str* points to a private/internal host.

    Returns None when the URL is safe to fetch.
    """
    try:
        parsed = urlparse(url_str)
    except Exception:
        return "invalid URL"

    if parsed.scheme not in _ALLOWED_URL_SCHEMES:
        return f"URL scheme '{parsed.scheme}' not allowed (use http or https)"

    hostname = parsed.hostname
    if not hostname:
        return "URL has no hostname"

    # Block localhost aliases
    if hostname.lower() in {"localhost", "127.0.0.1", "::1", "0.0.0.0"}:
        return f"URL host '{hostname}' is not allowed"

    # Block link-local / site-local IPv6
    if hostname.lower().startswith("fe80:") or hostname.lower() == "::1":
        return f"URL host '{hostname}' is not allowed"

    # Resolve and check against private blocks
    try:
        ip = ipaddress.ip_address(hostname)
    except ValueError:
        # Not an IP literal — do a DNS resolution check
        try:
            resolved = socket.getaddrinfo(hostname, None, family=socket.AF_INET)
        except socket.gaierror:
            return f"cannot resolve hostname: {hostname}"
        ips = {r[4][0] for r in resolved}
    else:
        ips = {str(ip)}

    for ip_str in ips:
        try:
            addr = ipaddress.ip_address(ip_str)
        except ValueError:
            continue
        if addr.is_loopback or addr.is_link_local or addr.is_multicast:
            return f"IP address {ip_str} is not allowed"
        for net in _PRIVATE_NETS:
            if addr in net:
                return f"IP address {ip_str} is private/internal — not allowed"

    return None  # safe


def _validate_tool_args(name: str, arguments: dict[str, Any]) -> str | None:
    """Validate tool arguments before execution.

    Returns an error string on failure, or None on success.
    """
    if name == "execute_python":
        code = arguments.get("code", "")
        if len(code) > _MAX_CODE_SIZE:
            return f"code too large ({len(code)} bytes, max {_MAX_CODE_SIZE})"

    elif name == "write_file":
        path = arguments.get("path", "")
        if not path:
            return "path is required"
        content = arguments.get("content", "")
        if len(content) > _MAX_FILE_CONTENT:
            return f"content too large ({len(content)} bytes, max {_MAX_FILE_CONTENT})"

    elif name == "read_file":
        path = arguments.get("path", "")
        if not path:
            return "path is required"

    elif name == "web_search":
        query = arguments.get("query", "")
        if not query:
            return "query is required"
        if len(query) > _MAX_SEARCH_QUERY:
            return f"query too long ({len(query)} chars, max {_MAX_SEARCH_QUERY})"

    elif name == "web_fetch":
        url = arguments.get("url", "")
        if not url:
            return "url is required"
        err = _check_ssrf(url)
        if err:
            return f"web_fetch rejected: {err}"

    elif name == "calculate":
        expression = arguments.get("expression", "")
        if not expression:
            return "expression is required"

    return None


# -- Path validation (symlink-aware) ------------------------------------


def _validate_path(path: str, workspace_dir: Path) -> Path:
    """Resolve *path* and verify it stays within the workspace.

    Differs from ``_resolve_path`` by also checking every path component
    for symlinks, preventing symlink-based escapes.
    """
    resolved = _resolve_path(path, workspace_dir)

    # Walk parent components and check for symlinks
    workspace_resolved = workspace_dir.resolve()
    for parent in [resolved, *resolved.parents]:
        try:
            parent.relative_to(workspace_resolved)
        except ValueError:
            break  # reached workspace boundary

        if parent.is_symlink():
            real = parent.resolve()
            try:
                real.relative_to(workspace_resolved)
            except ValueError as err:
                raise PermissionError(
                    f"Access denied: '{path}' contains a symlink pointing "
                    f"outside the workspace ({real})"
                ) from err

    return resolved


def execute_tool_call(
    call: dict[str, Any],
    workspace_dir: Path,
) -> dict[str, Any]:
    """Execute a single tool call locally and return a tool result message."""
    function = call.get("function") or {}
    name = str(function.get("name", ""))
    raw_arguments = function.get("arguments") or "{}"
    arguments = (
        json.loads(raw_arguments) if isinstance(raw_arguments, str) else raw_arguments
    )

    # Validate arguments before dispatching
    error = _validate_tool_args(name, arguments)
    if error:
        return {
            "role": "tool",
            "tool_call_id": call.get("id", ""),
            "name": name,
            "content": f"validation error: {error}",
        }

    if name == "execute_python":
        result = execute_python(arguments, workspace_dir, profile=profile_for_tool(name))
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
        # Check plugin registry before declaring unknown
        plugin_result = get_registry().execute(name, arguments, workspace_dir)
        if plugin_result is not None:
            result = plugin_result
        else:
            # Check MCP tools
            mcp_result = get_mcp_manager().execute_tool(name, arguments)
            if mcp_result is not None:
                result = mcp_result
            else:
                result = f"unknown tool: {name}"

    return {
        "role": "tool",
        "tool_call_id": call.get("id", ""),
        "name": name,
        "content": result if isinstance(result, str) else json.dumps(result, sort_keys=True),
    }


def get_tool_definitions() -> list[dict[str, Any]]:
    """Return merged tool definitions: built-in + plugins + MCP."""
    return [
        *TOOL_DEFINITIONS,
        *get_registry().get_definitions(),
        *get_mcp_manager().get_all_definitions(),
    ]


# -- Python execution (sandboxed) ---------------------------------------


def execute_python(
    arguments: dict[str, Any],
    workspace_dir: Path,
    profile: SandboxProfile | None = None,
) -> str:
    """Execute Python code in a sandboxed subprocess.

    Uses the configured sandbox backend (nsjail > bwrap > rlimit > none).
    The optional *profile* controls isolation level. See rova/sandbox.py.
    """
    code = arguments.get("code", "")
    if not code:
        return "error: no code provided"

    try:
        proc = get_sandbox().execute(code, profile=profile)
        if proc.returncode == 0:
            return proc.stdout
        if proc.returncode < 0:
            return f"error: process killed by signal {-proc.returncode}"
        return proc.stderr or f"error: exit code {proc.returncode}"
    except subprocess.TimeoutExpired:
        return "error: execution timed out (30s)"
    except FileNotFoundError:
        return "error: sandbox backend not available (missing executable)"
    except Exception as e:
        return str(e)


# -- File operations ----------------------------------------------------


def _make_diff(file_path: Path, new_content: str, context_lines: int = 3) -> str:
    """Generate a unified diff between the current file and proposed content.

    Returns an empty string if the file doesn't exist yet (new file).
    """
    if not file_path.is_file():
        return ""
    try:
        old_content = file_path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""

    if old_content == new_content:
        return ""

    import difflib
    old_lines = old_content.splitlines(keepends=True)
    new_lines = new_content.splitlines(keepends=True)
    diff = difflib.unified_diff(
        old_lines, new_lines,
        fromfile=str(file_path),
        tofile=str(file_path),
        n=context_lines,
    )
    return "".join(diff)


def write_file(
    arguments: dict[str, Any],
    workspace_dir: Path,
    *,
    dry_run: bool = False,
) -> str:
    """Write content to a file in the workspace.

    If *dry_run* is True, returns the diff without writing.
    If the file already exists, returns a unified diff of the changes.
    For new files, returns a creation notice.
    """
    path = arguments.get("path", "")
    content = arguments.get("content", "")
    try:
        file_path = _validate_path(path, workspace_dir)
        file_path.parent.mkdir(parents=True, exist_ok=True)

        diff = _make_diff(file_path, content)

        if dry_run:
            if diff:
                return f"diff for {file_path}:\n{diff}"
            if not file_path.is_file():
                return f"would create new file: {file_path} ({len(content)} bytes)"
            return f"no changes for {file_path}"

        # Actually write
        if diff:
            is_new = False
        else:
            is_new = not file_path.is_file()

        file_path.write_text(content, encoding="utf-8")

        if is_new:
            return f"created {file_path} ({len(content)} bytes)"
        if diff:
            return f"wrote {len(content)} bytes to {file_path} (diff above)"
        return f"wrote {len(content)} bytes to {file_path}"
    except Exception as e:
        return str(e)


def read_file(arguments: dict[str, Any], workspace_dir: Path) -> str:
    path = arguments.get("path", "")
    try:
        file_path = _validate_path(path, workspace_dir)
        # Guard against reading huge files
        if file_path.stat().st_size > _MAX_FILE_READ:
            return f"error: file too large ({file_path.stat().st_size} bytes, max {_MAX_FILE_READ})"
        return file_path.read_text(encoding="utf-8", errors="replace")
    except Exception as e:
        return str(e)


def list_files(arguments: dict[str, Any], workspace_dir: Path) -> str:
    path = arguments.get("path", ".")
    try:
        target = _validate_path(path, workspace_dir)
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
    except ValueError as err:
        raise PermissionError(
            f"Access denied: '{path}' resolves outside the workspace ({workspace_resolved})"
        ) from err

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
            timeout=WEB_SEARCH_TIMEOUT,
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
    max_length = arguments.get("max_length", WEB_FETCH_MAX_CHARS)
    if not url:
        return "error: url is required"

    try:
        response = httpx.get(
            url,
            timeout=WEB_FETCH_TIMEOUT,
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

    for i, title in enumerate(titles[:WEB_SEARCH_MAX_RESULTS]):
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
