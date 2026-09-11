"""Web helpers for tools — SSRF checks, HTML stripping, DDG parsing.

Extracted from ``r105/tools.py``. Pure helpers with no registry imports.
"""

from __future__ import annotations

import html.parser
import ipaddress
import re
import socket
from urllib.parse import urlparse

_ALLOWED_URL_SCHEMES = {"http", "https"}


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
            resolved = socket.getaddrinfo(hostname, None)
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
        # Unwrap IPv4-mapped IPv6 (e.g. ::ffff:10.0.0.1) so an embedded
        # private IPv4 address cannot slip through the family checks below.
        if isinstance(addr, ipaddress.IPv6Address) and addr.ipv4_mapped is not None:
            addr = addr.ipv4_mapped
        if addr.is_loopback or addr.is_link_local or addr.is_multicast:
            return f"IP address {ip_str} is not allowed"
        if addr.is_private or addr.is_reserved or addr.is_unspecified or not addr.is_global:
            return f"IP address {ip_str} is private/internal — not allowed"
    return None


class HTMLStripper(html.parser.HTMLParser):
    """Structural HTML stripper that extracts readable text."""

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
        raw = re.sub(r'[ \t]+', ' ', raw)
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
