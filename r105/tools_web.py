"""Web helpers for tools — SSRF checks, HTML stripping, DDG parsing,
and the registered ``web_search`` / ``web_fetch`` tool implementations.

Pure helpers plus thin registry wiring; no executor imports (no cycle with
``r105.tools`` — both import the registry from ``r105.registry``).
"""

from __future__ import annotations

import html.parser
import ipaddress
import json
import re
import socket
import ssl
from collections.abc import Iterable
from typing import Any
from urllib.parse import urljoin, urlparse

import httpcore
import httpx

from r105 import __version__
from r105.constants import (
    WEB_FETCH_MAX_CHARS,
    WEB_FETCH_TIMEOUT,
    WEB_SEARCH_MAX_RESULTS,
    WEB_SEARCH_TIMEOUT,
)
from r105.registry import get_tool_registry

_USER_AGENT = f"r105/{__version__}"

_ALLOWED_URL_SCHEMES = {"http", "https"}
_REDIRECT_STATUSES = {301, 302, 303, 307, 308}
_MAX_REDIRECTS = 5


def _validate_ip_address(ip_str: str) -> str | None:
    """Return an error when *ip_str* is not a public routable address."""
    try:
        addr = ipaddress.ip_address(ip_str)
    except ValueError:
        return f"hostname resolved to an invalid IP address: {ip_str!r}"
    # Unwrap IPv4-mapped IPv6 (e.g. ::ffff:10.0.0.1) so an embedded
    # private IPv4 address cannot slip through the family checks below.
    if isinstance(addr, ipaddress.IPv6Address) and addr.ipv4_mapped is not None:
        addr = addr.ipv4_mapped
    if addr.is_loopback or addr.is_link_local or addr.is_multicast:
        return f"IP address {ip_str} is not allowed"
    if addr.is_private or addr.is_reserved or addr.is_unspecified or not addr.is_global:
        return f"IP address {ip_str} is private/internal — not allowed"
    return None


def _validate_resolved_addresses(addresses: Iterable[str]) -> str | None:
    """Validate every address returned by one DNS resolution."""
    seen = False
    for ip_str in addresses:
        seen = True
        error = _validate_ip_address(ip_str)
        if error:
            return error
    if not seen:
        return "hostname resolved to no IP addresses"
    return None


def check_ssrf(url_str: str) -> str | None:
    """Return an error string if *url_str* points to a private/internal host.

    Returns None when the URL is safe to fetch. Both DNS families (A and
    AAAA) are resolved, and the checks are address-family agnostic so IPv6
    literals such as unique-local (``fc00::/7``) addresses are blocked too.
    Unresolvable hostnames fail closed.
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
    if hostname.lower() in {"localhost", "127.0.0.1", "::1", "0.0.0.0"}:
        return f"URL host '{hostname}' is not allowed"
    if hostname.lower().startswith("fe80:") or hostname.lower() == "::1":
        return f"URL host '{hostname}' is not allowed"
    try:
        ip = ipaddress.ip_address(hostname)
    except ValueError:
        try:
            resolved = socket.getaddrinfo(hostname, None, type=socket.SOCK_STREAM)
        except OSError:
            return f"cannot resolve hostname: {hostname}"
        ips = {str(r[4][0]) for r in resolved}
    else:
        ips = {str(ip)}
    return _validate_resolved_addresses(ips)


class _ValidatedSocketStream(httpcore.NetworkStream):
    """Small httpcore stream backed by a socket connected to a checked IP."""

    def __init__(self, sock: socket.socket) -> None:
        self._sock = sock

    def read(self, max_bytes: int, timeout: float | None = None) -> bytes:
        try:
            self._sock.settimeout(timeout)
            return self._sock.recv(max_bytes)
        except TimeoutError as exc:
            raise httpcore.ReadTimeout(str(exc)) from exc
        except OSError as exc:
            raise httpcore.ReadError(str(exc)) from exc

    def write(self, buffer: bytes, timeout: float | None = None) -> None:
        try:
            while buffer:
                self._sock.settimeout(timeout)
                written = self._sock.send(buffer)
                if written <= 0:
                    raise OSError("socket closed while writing")
                buffer = buffer[written:]
        except TimeoutError as exc:
            raise httpcore.WriteTimeout(str(exc)) from exc
        except OSError as exc:
            raise httpcore.WriteError(str(exc)) from exc

    def close(self) -> None:
        self._sock.close()

    def start_tls(
        self,
        ssl_context: ssl.SSLContext,
        server_hostname: str | None = None,
        timeout: float | None = None,
    ) -> httpcore.NetworkStream:
        try:
            self._sock.settimeout(timeout)
            self._sock = ssl_context.wrap_socket(
                self._sock,
                server_hostname=server_hostname,
            )
        except TimeoutError as exc:
            self.close()
            raise httpcore.ConnectTimeout(str(exc)) from exc
        except OSError as exc:
            self.close()
            raise httpcore.ConnectError(str(exc)) from exc
        return self

    def get_extra_info(self, info: str) -> Any:
        if info == "socket":
            return self._sock
        if info == "client_addr":
            return self._sock.getsockname()
        if info == "server_addr":
            return self._sock.getpeername()
        return None


class _SSRFProtectedNetworkBackend(httpcore.NetworkBackend):
    """Resolve once, validate every answer, and connect to that exact address.

    The stock httpcore backend passes the hostname to ``socket.create_connection``
    after the application-level SSRF check. That permits a DNS answer to change
    between the check and the TCP connection. This backend performs DNS itself,
    rejects any private answer, and calls ``socket.connect`` with the numeric
    sockaddr returned by that same resolution.
    """

    def connect_tcp(
        self,
        host: str,
        port: int,
        timeout: float | None = None,
        local_address: str | None = None,
        socket_options: Iterable[Any] | None = None,
    ) -> httpcore.NetworkStream:
        try:
            resolved = socket.getaddrinfo(
                host,
                port,
                type=socket.SOCK_STREAM,
            )
        except OSError as exc:
            raise httpcore.ConnectError(f"cannot resolve hostname: {host}") from exc
        if not resolved:
            raise httpcore.ConnectError(f"hostname resolved to no IP addresses: {host}")

        # Validate the complete answer set before attempting any connection.
        # A mixed public/private response fails closed just like check_ssrf().
        for _family, _socktype, _proto, _canonname, sockaddr in resolved:
            error = _validate_ip_address(str(sockaddr[0]))
            if error:
                raise httpcore.ConnectError(error)

        last_error: httpcore.ConnectError | httpcore.ConnectTimeout | None = None
        for family, socktype, proto, _canonname, sockaddr in resolved:
            sock = socket.socket(family, socktype or socket.SOCK_STREAM, proto)
            failed = False
            try:
                sock.settimeout(timeout)
                if local_address:
                    bind_address: tuple[Any, ...] = (
                        (local_address, 0, 0, 0)
                        if family == socket.AF_INET6
                        else (local_address, 0)
                    )
                    sock.bind(bind_address)
                if socket_options:
                    for option in socket_options:
                        sock.setsockopt(*option)
                if family in (socket.AF_INET, socket.AF_INET6):
                    sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
                # sockaddr contains the numeric DNS answer; no second lookup.
                sock.connect(sockaddr)
                return _ValidatedSocketStream(sock)
            except TimeoutError as exc:
                failed = True
                last_error = httpcore.ConnectTimeout(str(exc))
            except OSError as exc:
                failed = True
                last_error = httpcore.ConnectError(str(exc))
            finally:
                if failed and sock.fileno() != -1:
                    sock.close()
        raise last_error or httpcore.ConnectError(f"could not connect to {host}:{port}")

    def connect_unix_socket(
        self,
        path: str,
        timeout: float | None = None,
        socket_options: Iterable[Any] | None = None,
    ) -> httpcore.NetworkStream:
        del path, timeout, socket_options
        raise httpcore.ConnectError("Unix socket connections are disabled for web tools")


class SSRFProtectedTransport(httpx.HTTPTransport):
    """httpx transport that blocks private IPs at the TCP connection boundary."""

    def __init__(self) -> None:
        # Ambient HTTP(S)_PROXY values would move the connection boundary to a
        # proxy and hide the destination from this transport. Web tools use a
        # direct, explicitly checked connection instead.
        super().__init__(trust_env=False)
        pool = getattr(self, "_pool", None)
        if pool is None or not hasattr(pool, "_network_backend"):
            self.close()
            raise RuntimeError("unsupported httpx/httpcore transport internals")
        pool._network_backend = _SSRFProtectedNetworkBackend()


def _safe_http_client() -> httpx.Client:
    """Create a direct client with redirect handling delegated to our policy."""
    return httpx.Client(
        transport=SSRFProtectedTransport(),
        headers={"User-Agent": _USER_AGENT},
        follow_redirects=False,
        trust_env=False,
    )


def _request_with_validated_redirects(
    client: httpx.Client,
    url: str,
    *,
    params: dict[str, str] | None = None,
    timeout: float,
) -> httpx.Response:
    """GET *url* while checking every redirect before following it."""
    current_url = url
    current_params = params
    for _hop in range(_MAX_REDIRECTS + 1):
        error = check_ssrf(current_url)
        if error:
            raise ValueError(error)
        response = client.get(
            current_url,
            params=current_params,
            timeout=timeout,
            follow_redirects=False,
        )
        current_params = None
        if response.status_code not in _REDIRECT_STATUSES:
            response.raise_for_status()
            return response
        location = response.headers.get("location")
        if not location:
            response.raise_for_status()
            return response
        next_url = urljoin(str(response.url), location)
        next_error = check_ssrf(next_url)
        response.close()
        if next_error:
            raise ValueError(f"redirect blocked: {next_error}")
        current_url = next_url
    raise ValueError(f"too many redirects (max {_MAX_REDIRECTS})")


class HTMLStripper(html.parser.HTMLParser):
    """Structural HTML stripper that extracts readable text."""

    BLOCK_TAGS = {
        "div", "p", "br", "li", "h1", "h2", "h3", "h4", "h5", "h6",
        "tr", "article", "section", "header", "footer", "nav", "main",
        "ul", "ol", "dl", "table", "blockquote", "pre", "hr", "form",
        "fieldset", "figure", "figcaption", "details", "summary",
    }
    SKIP_TAGS = {"script", "style", "noscript", "head", "meta", "link", "title"}
    SKIP_CONTAINER_TAGS = {"script", "style", "noscript", "head", "title"}

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self._parts: list[str] = []
        self._skip_depth = 0

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag in self.SKIP_CONTAINER_TAGS:
            self._skip_depth += 1

    def handle_endtag(self, tag: str) -> None:
        if tag in self.SKIP_CONTAINER_TAGS and self._skip_depth > 0:
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
        raw = re.sub(r'[ \t]+', ' ', raw)
        raw = re.sub(r'[ \t]+\n', '\n', raw)
        raw = re.sub(r'\n\s*\n', '\n\n', raw)
        return raw.strip()


def strip_html(html: str) -> str:
    """Strip HTML tags and return plain text using a structural parser."""
    stripper = HTMLStripper()
    try:
        stripper.feed(html)
        stripper.close()
        return stripper.get_text()
    except Exception:
        text = re.sub(r'<script[^>]*>.*?</script>', '', html, flags=re.DOTALL | re.IGNORECASE)
        text = re.sub(r'<style[^>]*>.*?</style>', '', text, flags=re.DOTALL | re.IGNORECASE)
        text = re.sub(r'<[^>]+>', ' ', text)
        text = re.sub(r'[ \t]+', ' ', text)
        text = re.sub(r'\n\s*\n', '\n\n', text)
        return text.strip()


def clean_html(text: str) -> str:
    return strip_html(text)


def parse_ddg_results(html: str, max_results: int = 10) -> list[dict[str, str]]:
    """Extract search results from DuckDuckGo HTML response."""
    results: list[dict[str, str]] = []
    title_pattern = re.compile(r'<a[^>]*class="result__a"[^>]*>(.*?)</a>', re.DOTALL | re.IGNORECASE)
    snippet_pattern = re.compile(r'<a[^>]*class="result__snippet"[^>]*>(.*?)</a>', re.DOTALL | re.IGNORECASE)
    url_pattern = re.compile(r'<a[^>]*class="result__url"[^>]*>(.*?)</a>', re.DOTALL | re.IGNORECASE)
    titles = title_pattern.findall(html)
    snippets = snippet_pattern.findall(html)
    urls = url_pattern.findall(html)
    for i, title in enumerate(titles[:max_results]):
        results.append({
            "title": clean_html(title),
            "url": clean_html(urls[i]) if i < len(urls) else "",
            "snippet": clean_html(snippets[i]) if i < len(snippets) else "",
        })
    return results


@get_tool_registry().register(
    name="web_search",
    description="Search the web and return results with titles, URLs, and snippets.",
    parameters={"query": {"type": "string", "description": "Search query string."}},
    required=["query"],
    needs_network=True,
    needs_filesystem=False,
    needs_output_truncation=True,
)
def web_search(arguments: dict[str, Any]) -> str:
    """Search the web using DuckDuckGo HTML (no API key required)."""
    query = arguments.get("query", "")
    if not query:
        return "error: query is required"

    try:
        with _safe_http_client() as client:
            response = _request_with_validated_redirects(
                client,
                "https://html.duckduckgo.com/html/",
                params={"q": query},
                timeout=WEB_SEARCH_TIMEOUT,
            )
            results = parse_ddg_results(response.text, WEB_SEARCH_MAX_RESULTS)
        if not results:
            return f"no results found for: {query}"
        return json.dumps(results, indent=2, ensure_ascii=False)
    except httpx.HTTPError as e:
        return f"search error: {e}"
    except Exception as e:
        return f"search error: {e}"


@get_tool_registry().register(
    name="web_fetch",
    description="Fetch a URL and return its text content (HTML tags removed).",
    parameters={
        "url": {"type": "string", "description": "URL to fetch."},
        "max_length": {"type": "integer", "description": "Maximum characters to return (default: 8000)."},
    },
    required=["url"],
    needs_network=True,
    needs_filesystem=False,
    needs_output_truncation=True,
    needs_external_wrapping=True,
)
def web_fetch(arguments: dict[str, Any]) -> str:
    """Fetch a URL and return its text content (HTML tags stripped)."""
    url = arguments.get("url", "")
    max_length = arguments.get("max_length", WEB_FETCH_MAX_CHARS)
    if not url:
        return "error: url is required"

    try:
        with _safe_http_client() as client:
            response = _request_with_validated_redirects(
                client,
                url,
                timeout=WEB_FETCH_TIMEOUT,
            )
            text = strip_html(response.text)
        if len(text) > max_length:
            text = text[:max_length] + f"\n... (truncated, original: {len(text)} chars)"
        return text
    except httpx.HTTPError as e:
        return f"fetch error: {e}"
    except Exception as e:
        return f"fetch error: {e}"
