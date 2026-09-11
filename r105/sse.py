"""SSE streaming core — shared by all backends.

``stream_sse()`` takes an explicit connection description (URL, headers,
timeout) instead of a client object so it stays independent of the backend
class hierarchy in ``r105.backends``.
"""

from __future__ import annotations

import asyncio
import json
import time
from collections.abc import Callable
from typing import Any

import httpx

from r105.errors import RouterAPIError
from r105.logging import error as log_error
from r105.model_catalog import uses_gemma4_channel_syntax
from r105.payload import (
    _GEMMA4_TOOL_CALL_RE,
    _parse_gemma4_tool_calls,
    _parse_response,
)
from r105.state import ChatResult

# Retry policy for transient pre-stream failures (connect errors, timeouts,
# HTTP 5xx before the first byte). Once streaming has started (content
# buffered) failures are raised immediately — the backend has no resume
# protocol, so a mid-stream retry would duplicate or lose tokens.
SSE_MAX_ATTEMPTS = 3
SSE_RETRY_BASE_SECONDS = 0.5


async def stream_sse(
    payload: dict[str, Any],
    *,
    url: str,
    headers: dict[str, str],
    timeout: httpx.Timeout,
    client: httpx.AsyncClient | None,
    on_chunk: Callable[[str], None],
    on_status: Callable[[str], None] | None = None,
    config_families: dict[str, str | None] | None = None,
) -> ChatResult:
    """Stream a chat completion over SSE and return the assembled result.

    ``config_families`` (from the ``model_families`` config key) is passed
    to the family gate so config-driven family overrides also apply to
    native Gemma-4 tool-call parsing.

    Robustness contract:
    - ``event:`` lines are tracked per the SSE spec; an ``event: error``
      frame raises :class:`RouterAPIError` with the server payload.
    - Malformed ``data:`` frames are skipped and counted (logged with
      context); a truncated stream returns whatever was buffered.
    - Transient pre-stream failures (connect errors, timeouts, HTTP 5xx)
      are retried with exponential backoff up to ``SSE_MAX_ATTEMPTS``.
      Mid-stream failures are raised — there is no resume protocol.
    """
    started = time.perf_counter()

    content_parts: list[str] = []
    reasoning_parts: list[str] = []
    tool_call_deltas: dict[int, dict[str, Any]] = {}
    malformed_lines = 0

    def _note_malformed(line: str) -> None:
        nonlocal malformed_lines
        malformed_lines += 1
        if malformed_lines <= 3 or malformed_lines % 25 == 0:
            log_error("sse_malformed_line", line=line[:200], count=malformed_lines)

    async def _read(http: httpx.AsyncClient) -> None:
        nonlocal content_parts, reasoning_parts, tool_call_deltas
        pending_event = "message"
        async with http.stream(
            "POST",
            url,
            json=payload,
            headers=headers,
            timeout=timeout,
        ) as response:
            if response.is_error:
                # NOTE: _check_response() cannot be used here — it reads
                # ``response.text``, which raises ResponseNotRead on an
                # unread streaming response. Read the error body first.
                body = await response.aread()
                try:
                    body_text = body.decode("utf-8", "replace")
                except Exception:
                    body_text = ""
                log_error(
                    "api_error",
                    status_code=response.status_code,
                    url=str(response.url),
                )
                raise RouterAPIError(
                    f"API error {response.status_code}: {response.reason_phrase}",
                    status_code=response.status_code,
                    response_body=body_text[:500],
                )
            async for line in response.aiter_lines():
                if not line:
                    # Blank line = SSE dispatch boundary; reset event type.
                    pending_event = "message"
                    continue
                if line.startswith(":"):
                    continue  # SSE comment / heartbeat
                if line.startswith("event:"):
                    pending_event = line[6:].strip() or "message"
                    continue
                if not line.startswith("data:"):
                    continue
                data = line[5:]
                if data.startswith(" "):
                    data = data[1:]
                if data == "[DONE]":
                    break
                if pending_event == "error":
                    pending_event = "message"
                    raise RouterAPIError(
                        f"Stream error from backend: {data[:500]}",
                        status_code=response.status_code,
                        response_body=data[:500],
                    )

                try:
                    chunk = json.loads(data)
                except json.JSONDecodeError:
                    _note_malformed(data)
                    continue
                if not isinstance(chunk, dict):
                    _note_malformed(data)
                    continue

                choices = chunk.get("choices")
                if not isinstance(choices, list) or not choices:
                    continue
                first = choices[0]
                if not isinstance(first, dict):
                    _note_malformed(data)
                    continue
                delta = first.get("delta") or {}
                if not isinstance(delta, dict):
                    _note_malformed(data)
                    continue

                content_delta = delta.get("content", "")
                if isinstance(content_delta, str) and content_delta:
                    content_parts.append(content_delta)
                    on_chunk(content_delta)

                reasoning_delta = delta.get("reasoning_content", "")
                if isinstance(reasoning_delta, str) and reasoning_delta:
                    reasoning_parts.append(reasoning_delta)

                tc_deltas = delta.get("tool_calls") or []
                if not isinstance(tc_deltas, list):
                    _note_malformed(data)
                    continue
                for tc in tc_deltas:
                    if not isinstance(tc, dict):
                        continue
                    idx = tc.get("index", 0)
                    if not isinstance(idx, int):
                        continue
                    if idx not in tool_call_deltas:
                        tool_call_deltas[idx] = {
                            "id": tc.get("id", ""),
                            "type": "function",
                            "function": {"name": "", "arguments": ""},
                        }
                    entry = tool_call_deltas[idx]
                    if tc.get("id"):
                        entry["id"] = tc["id"]
                    func = tc.get("function") or {}
                    if not isinstance(func, dict):
                        continue
                    if func.get("name"):
                        entry["function"]["name"] += func["name"]
                    if func.get("arguments"):
                        entry["function"]["arguments"] += func["arguments"]

    def _stream_started() -> bool:
        return bool(content_parts or reasoning_parts or tool_call_deltas)

    async def _backoff(attempt: int, reason: str) -> None:
        delay = SSE_RETRY_BASE_SECONDS * (2 ** (attempt - 1))
        log_error("sse_retry", attempt=attempt, delay_seconds=delay, reason=reason)
        if on_status is not None:
            on_status(
                f"Retrying backend (attempt {attempt + 1}/{SSE_MAX_ATTEMPTS}) "
                f"in {delay:.1f}s…"
            )
        await asyncio.sleep(delay)

    for attempt in range(1, SSE_MAX_ATTEMPTS + 1):
        try:
            if client is not None:
                await _read(client)
            else:
                async with httpx.AsyncClient() as ac:
                    await _read(ac)
            break
        except RouterAPIError as exc:
            transient = exc.status_code is not None and 500 <= exc.status_code <= 599
            if transient and not _stream_started() and attempt < SSE_MAX_ATTEMPTS:
                await _backoff(attempt, f"http_{exc.status_code}")
                continue
            raise
        except (httpx.ConnectError, httpx.TimeoutException) as exc:
            if not _stream_started() and attempt < SSE_MAX_ATTEMPTS:
                await _backoff(attempt, type(exc).__name__)
                continue
            raise

    if malformed_lines:
        log_error("sse_malformed_summary", count=malformed_lines)

    content = "".join(content_parts)
    tool_calls = [tool_call_deltas[i] for i in sorted(tool_call_deltas)]

    # Model-agnostic fallback: when the model produced no content (e.g. it
    # exhausted its token budget during reasoning) and made no tool calls,
    # surface the reasoning instead of an empty reply.
    if not content and not tool_calls and reasoning_parts:
        content = "".join(reasoning_parts)

    # Gemma-4-family models: parse native <|tool_call|> blocks from content
    # when the backend returns them inline instead of OpenAI-compatible
    # delta.tool_calls (e.g. llama.cpp without --jinja). Strictly gated on
    # model family — for every other model the content is opaque text and
    # is never regex-interpreted.
    if (
        not tool_calls
        and uses_gemma4_channel_syntax(
            str(payload.get("model", "")), config_families
        )
    ):
        native_calls = _parse_gemma4_tool_calls(content)
        if native_calls:
            tool_calls = native_calls
            content = _GEMMA4_TOOL_CALL_RE.sub("", content).strip()

    raw: dict[str, Any] = {
        "choices": [{
            "message": {
                "content": content,
                "tool_calls": tool_calls if tool_calls else None,
            }
        }],
        "timings": {},
    }
    return _parse_response(raw, started)
