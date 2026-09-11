"""SSRF prevention tests for r105.tools_web.check_ssrf (used by web_fetch)."""

import socket

import httpx
import pytest

from r105.tools_web import (
    _request_with_validated_redirects,
    _SSRFProtectedNetworkBackend,
    check_ssrf,
    strip_html,
    web_fetch,
)


class TestLiteralHosts:
    def test_public_ipv4_literal_passes(self):
        assert check_ssrf("https://93.184.216.34/") is None

    @pytest.mark.parametrize("host", [
        "localhost", "127.0.0.1", "::1", "0.0.0.0",
        "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254",
    ])
    def test_private_ipv4_blocked(self, host):
        assert check_ssrf(f"http://{host}/") is not None

    @pytest.mark.parametrize("host", [
        "[::1]", "[fe80::1]", "[fc00::1]", "[fd00::dead:beef]",
        "[::ffff:127.0.0.1]", "[::ffff:10.0.0.1]",
    ])
    def test_private_ipv6_blocked(self, host):
        assert check_ssrf(f"http://{host}/x") is not None

    def test_public_ipv6_literal_passes(self):
        assert check_ssrf("https://[2606:2800:220:1:248:1893:25c8:1946]/") is None


class TestSchemeAndShape:
    def test_non_http_scheme_blocked(self):
        assert check_ssrf("file:///etc/passwd") is not None
        assert check_ssrf("ftp://example.com/x") is not None

    def test_missing_hostname_blocked(self):
        assert check_ssrf("http://") is not None

    def test_public_hostname_passes_without_dns(self, monkeypatch):
        # example.com resolves publicly; stub DNS to avoid network in tests.
        monkeypatch.setattr(
            socket, "getaddrinfo",
            lambda *a, **k: [(socket.AF_INET, None, None, "", ("93.184.216.34", 0))],
        )
        assert check_ssrf("https://example.com/") is None


class TestDnsResolution:
    def test_dns_private_ipv4_blocked(self, monkeypatch):
        monkeypatch.setattr(
            socket, "getaddrinfo",
            lambda *a, **k: [(socket.AF_INET, None, None, "", ("10.9.9.9", 0))],
        )
        assert check_ssrf("http://internal.example/") is not None

    def test_dns_private_ipv6_blocked(self, monkeypatch):
        # AAAA-only host pointing at unique-local space must not slip through.
        monkeypatch.setattr(
            socket, "getaddrinfo",
            lambda *a, **k: [(socket.AF_INET6, None, None, "", ("fd00::99", 0, 0, 0))],
        )
        assert check_ssrf("http://internal6.example/") is not None

    def test_dns_mixed_families_one_private_blocked(self, monkeypatch):
        monkeypatch.setattr(
            socket, "getaddrinfo",
            lambda *a, **k: [
                (socket.AF_INET, None, None, "", ("93.184.216.34", 0)),
                (socket.AF_INET6, None, None, "", ("::1", 0, 0, 0)),
            ],
        )
        assert check_ssrf("http://mixed.example/") is not None

    def test_unresolvable_fails_closed(self, monkeypatch):
        def _raise(*a, **k):
            raise socket.gaierror("no such host")
        monkeypatch.setattr(socket, "getaddrinfo", _raise)
        assert check_ssrf("http://does-not-exist.invalid/") is not None

    def test_both_families_queried(self, monkeypatch):
        seen = {}

        def _fake(hostname, port, family=0, *a, **k):
            seen["family"] = family
            return [(socket.AF_INET, None, None, "", ("93.184.216.34", 0))]

        monkeypatch.setattr(socket, "getaddrinfo", _fake)
        assert check_ssrf("https://example.com/") is None
        # family=0 (AF_UNSPEC) queries A and AAAA; must not pin AF_INET.
        assert seen["family"] in (0, socket.AF_UNSPEC)


class TestFetchBoundary:
    def test_html_strip_does_not_treat_void_head_tags_as_containers(self):
        html = (
            "<head><title>Hidden</title><meta name='description' content='x'>"
            "<link rel='stylesheet' href='x'></head>"
            "<body><h1>Visible</h1><p>Content</p></body>"
        )
        assert strip_html(html) == "Visible\nContent"

    def test_transport_connects_to_the_checked_address_after_fallback(self, monkeypatch):
        import r105.tools_web as web_tools

        resolved = [
            (socket.AF_INET, socket.SOCK_STREAM, 0, "", ("93.184.216.34", 80)),
            (socket.AF_INET, socket.SOCK_STREAM, 0, "", ("93.184.216.35", 80)),
        ]
        monkeypatch.setattr(socket, "getaddrinfo", lambda *a, **k: resolved)

        sockets = []

        class FakeSocket:
            def __init__(self):
                self.connected = None
                self.closed = False
                sockets.append(self)

            def settimeout(self, _timeout):
                pass

            def setsockopt(self, *_args):
                pass

            def connect(self, address):
                if address[0] == "93.184.216.34":
                    raise OSError("first address unavailable")
                self.connected = address

            def fileno(self):
                return -1 if self.closed else 1

            def close(self):
                self.closed = True

        monkeypatch.setattr(web_tools.socket, "socket", lambda *a, **k: FakeSocket())
        stream = _SSRFProtectedNetworkBackend().connect_tcp("public.example", 80)
        assert len(sockets) == 2
        assert sockets[1].connected == ("93.184.216.35", 80)
        assert sockets[1].closed is False
        stream.close()

    def test_redirect_to_private_host_is_blocked_before_second_request(self, monkeypatch):
        monkeypatch.setattr(
            socket,
            "getaddrinfo",
            lambda *a, **k: [
                (socket.AF_INET, socket.SOCK_STREAM, 0, "", ("93.184.216.34", 0))
            ],
        )

        def handler(request: httpx.Request) -> httpx.Response:
            return httpx.Response(
                302,
                headers={"location": "http://127.0.0.1/admin"},
                request=request,
            )

        with httpx.Client(
            transport=httpx.MockTransport(handler),
            follow_redirects=False,
        ) as client, pytest.raises(ValueError, match="redirect blocked"):
            _request_with_validated_redirects(
                client,
                "http://public.example/start",
                timeout=1.0,
            )

    def test_dns_rebinding_is_blocked_at_connection_boundary(self, monkeypatch):
        calls = 0

        def fake_getaddrinfo(*args, **kwargs):
            nonlocal calls
            calls += 1
            address = "93.184.216.34" if calls == 1 else "127.0.0.1"
            return [(socket.AF_INET, socket.SOCK_STREAM, 0, "", (address, args[1]))]

        monkeypatch.setattr(socket, "getaddrinfo", fake_getaddrinfo)
        result = web_fetch({"url": "http://rebind.example/"})
        assert calls >= 2
        assert "not allowed" in result.lower() or "private/internal" in result.lower()
