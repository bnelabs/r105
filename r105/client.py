"""Backend-agnostic HTTP client — OpenAI-compatible, Router, and Ollama.

r105 can connect to any OpenAI-compatible API (OpenAI, Ollama, vLLM, etc.),
to a llama-router backend (with profiles), or to Ollama's native API.

Auto-detection order:
1. R105_URL / --url set → RouterClient (has profiles)
2. OPENAI_API_KEY set → DirectClient (OpenAI-compatible)
3. otherwise → check local Ollama, then fall back to DirectClient

All backends share the same core chat methods:
- send() / async_send()        — one-shot
- async_continue()             — tool-loop continuation
- async_send_streaming()       — SSE streaming
- compact() / async_compact()  — summarization

This module is a backwards-compatibility facade. The implementation lives in
:mod:`r105.backends` (client classes), :mod:`r105.payload` (wire format),
and :mod:`r105.sse` (streaming core).
"""

from __future__ import annotations

from r105.backends import (
    BACKEND_DIRECT,
    BACKEND_ROUTER,
    BackendCapabilities,
    BaseClient,
    DirectClient,
    RouterClient,
    create_client,
)
from r105.payload import (
    _build_payload,
    _extract_assistant_content,
    _extract_tool_calls,
    _inject_reasoning_effort,
    _maybe_float,
    _metadata_from_state,
    _parse_gemma4_tool_calls,
    _parse_response,
)

__all__ = [
    "BACKEND_DIRECT",
    "BACKEND_ROUTER",
    "BackendCapabilities",
    "BaseClient",
    "DirectClient",
    "RouterClient",
    "_build_payload",
    "_extract_assistant_content",
    "_extract_tool_calls",
    "_inject_reasoning_effort",
    "_maybe_float",
    "_metadata_from_state",
    "_parse_gemma4_tool_calls",
    "_parse_response",
    "create_client",
]
