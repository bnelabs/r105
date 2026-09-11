"""Backend clients — OpenAI-compatible (direct) and llama-router (router).

``r105.client`` re-exports this package's public surface for backwards
compatibility; new code may import from ``r105.backends`` directly.
"""

from __future__ import annotations

import os

from r105.backends.base import BackendCapabilities, BaseClient
from r105.backends.direct import DirectClient
from r105.backends.router import RouterClient
from r105.constants import DEFAULT_HTTP_TIMEOUT

BACKEND_ROUTER = "router"
BACKEND_DIRECT = "direct"


def create_client(
    base_url: str | None = None,
    backend: str | None = None,
    timeout: float = DEFAULT_HTTP_TIMEOUT,
) -> BaseClient:
    """Create the best available backend client.

    Detection order (when *backend* is not specified):
    1. If *base_url* is explicitly provided → RouterClient at that URL
    2. If R105_URL env var is set → RouterClient
    3. If OPENAI_API_KEY env var is set → DirectClient
    4. Otherwise → DirectClient at OPENAI_BASE_URL or default

    When *backend* is ``"router"``, force RouterClient (fails if unreachable).
    When *backend* is ``"direct"``, force DirectClient (no profiles).
    """
    explicit_url = base_url is not None and base_url != ""

    if backend == "router" or (backend is None and explicit_url):
        return RouterClient(base_url=base_url, timeout=timeout)

    if backend == "direct":
        return DirectClient(base_url=base_url, timeout=timeout)

    # Auto-detect
    router_url = base_url or os.environ.get("R105_URL")
    if router_url:
        return RouterClient(base_url=router_url, timeout=timeout)

    # Fallback to OpenAI-compatible
    return DirectClient(base_url=base_url, timeout=timeout)


__all__ = [
    "BACKEND_DIRECT",
    "BACKEND_ROUTER",
    "BackendCapabilities",
    "BaseClient",
    "DirectClient",
    "RouterClient",
    "create_client",
]
