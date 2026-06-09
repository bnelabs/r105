"""HTTP client for llama-router API — sync and async support."""

from __future__ import annotations

import time
from typing import Any

import httpx

from rova.state import (
    DEFAULT_MODEL,
    ChatResult,
    ChatState,
)


def _metadata_from_state(state: ChatState) -> dict[str, Any]:
    metadata: dict[str, Any] = {}
    if state.profile:
        metadata["profile"] = state.profile
    if state.rag is not None:
        metadata["rag"] = state.rag
    if state.quality:
        metadata["quality"] = state.quality
    return metadata


def _skill_messages(state: ChatState) -> list[dict[str, str]]:
    from rova.skills import read_skill

    messages: list[dict[str, str]] = []
    for name in state.active_skills:
        params = state.skill_params.get(name)
        text = read_skill(state.skills_dir, name, params)
        if text:
            messages.append({"role": "system", "content": f"Active skill: {name}\n{text}"})
    return messages


def _extract_assistant_content(raw: dict[str, Any]) -> str:
    choices = raw.get("choices") or []
    if not choices:
        return ""
    message = choices[0].get("message") or {}
    content = message.get("content", "")
    if isinstance(content, str):
        return content.strip()
    return str(content).strip()


def _extract_tool_calls(raw: dict[str, Any]) -> list[dict[str, Any]]:
    choices = raw.get("choices") or []
    if not choices:
        return []
    message = choices[0].get("message") or {}
    calls = message.get("tool_calls") or []
    return [call for call in calls if isinstance(call, dict)]


def _maybe_float(value: Any) -> float | None:
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


def _build_payload(message: str, state: ChatState, tools: list[dict[str, Any]] | None = None) -> dict[str, Any]:
    """Build the request payload for a chat completion."""
    messages = [*_skill_messages(state), *state.history, {"role": "user", "content": message}]
    payload: dict[str, Any] = {
        "model": DEFAULT_MODEL,
        "messages": messages,
        "stream": False,
    }
    metadata = _metadata_from_state(state)
    if metadata:
        payload["metadata"] = metadata
    if state.max_tokens is not None:
        payload["max_tokens"] = state.max_tokens
    if state.json_mode:
        payload["response_format"] = {"type": "json_object"}
    if tools:
        payload["tools"] = tools
        payload["tool_choice"] = "auto"
    return payload


def _parse_response(raw: dict[str, Any], started: float) -> ChatResult:
    """Parse an API response into a ChatResult."""
    wall_seconds = time.perf_counter() - started
    content = _extract_assistant_content(raw)
    tool_calls = _extract_tool_calls(raw)
    timings = raw.get("timings") or {}
    return ChatResult(
        content=content,
        wall_seconds=wall_seconds,
        prompt_tps=_maybe_float(timings.get("prompt_per_second")),
        generation_tps=_maybe_float(timings.get("predicted_per_second")),
        raw=raw,
        tool_calls=tool_calls,
    )


class RouterClient:
    """HTTP client for the llama-router API.

    Supports both sync (for non-interactive use) and async (for TUI use).
    """

    def __init__(
        self,
        base_url: str = "http://127.0.0.1:8010",
        timeout: float = 300.0,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self.timeout = timeout

    # -- Sync API (for CLI subcommands) -----------------------------------

    def health(self) -> dict[str, Any]:
        response = httpx.get(f"{self.base_url}/health", timeout=10.0)
        response.raise_for_status()
        return response.json()

    def profiles(self) -> dict[str, Any]:
        response = httpx.get(f"{self.base_url}/profiles", timeout=10.0)
        response.raise_for_status()
        return response.json()

    def ingest(self, paths: list[str] | None = None, urls: list[str] | None = None) -> dict[str, Any]:
        payload: dict[str, Any] = {"paths": paths or [], "urls": urls or []}
        response = httpx.post(f"{self.base_url}/rag/ingest", json=payload, timeout=self.timeout)
        response.raise_for_status()
        return response.json()

    def search(self, query: str, top_k: int = 5) -> dict[str, Any]:
        response = httpx.post(
            f"{self.base_url}/rag/search",
            json={"query": query, "top_k": top_k},
            timeout=self.timeout,
        )
        response.raise_for_status()
        return response.json()

    def list_rag_documents(self) -> dict[str, Any]:
        """List all documents in the RAG index."""
        response = httpx.get(f"{self.base_url}/rag/documents", timeout=10.0)
        response.raise_for_status()
        return response.json()

    def delete_rag_document(self, doc_id: str) -> dict[str, Any]:
        """Delete a document from the RAG index by ID."""
        response = httpx.delete(
            f"{self.base_url}/rag/documents/{doc_id}", timeout=10.0
        )
        response.raise_for_status()
        return response.json()

    def send(
        self,
        message: str,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
    ) -> ChatResult:
        """Send a message synchronously and return the result."""
        payload = _build_payload(message, state, tools)
        started = time.perf_counter()
        response = httpx.post(
            f"{self.base_url}/v1/chat/completions",
            json=payload,
            timeout=self.timeout,
        )
        response.raise_for_status()
        raw = response.json()
        result = _parse_response(raw, started)
        state.history.extend([
            {"role": "user", "content": message},
            {"role": "assistant", "content": result.content},
        ])
        return result

    def compact(self, state: ChatState) -> ChatResult:
        """Summarize conversation history and replace it with the summary.

        Summarizes the older ~70% of messages while retaining the most recent
        ~30% as raw context so the LLM doesn't lose recent tool calls/results.
        """
        if not state.history:
            return ChatResult(
                content="No conversation history to compact.",
                wall_seconds=0,
                prompt_tps=None,
                generation_tps=None,
                raw={},
            )
        # Keep the most recent ~30% of messages; summarize the older ~70%
        split = max(1, len(state.history) - max(4, len(state.history) // 3))
        older = state.history[:split]
        recent = state.history[split:]

        transcript = "\n\n".join(
            f"{message['role']}: {message['content']}" for message in older
        )
        prompt = (
            "Compact the conversation below into a durable summary for continuing the same chat. "
            "Preserve user goals, decisions, constraints, important facts, open questions, file paths, "
            "commands, and unresolved work. Remove filler and repeated wording. Return only the summary.\n\n"
            f"{transcript}"
        )
        compact_state = ChatState(
            profile="complex_reasoning",
            quality="balanced",
            max_tokens=2048,
            skills_dir=state.skills_dir,
            context_tokens=state.context_tokens,
        )
        result = self.send(prompt, compact_state)
        # Replace history: summary first, then retain recent raw messages
        state.history = [
            {"role": "system", "content": f"Conversation summary so far:\n{result.content}"},
            *recent,
        ]
        return result

    # -- Async API (for TUI) -----------------------------------------------

    async def async_send(
        self,
        message: str,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
        client: httpx.AsyncClient | None = None,
    ) -> ChatResult:
        """Send a message asynchronously using an optional shared AsyncClient."""
        payload = _build_payload(message, state, tools)
        started = time.perf_counter()

        if client is not None:
            response = await client.post(
                f"{self.base_url}/v1/chat/completions",
                json=payload,
                timeout=self.timeout,
            )
        else:
            async with httpx.AsyncClient() as ac:
                response = await ac.post(
                    f"{self.base_url}/v1/chat/completions",
                    json=payload,
                    timeout=self.timeout,
                )

        response.raise_for_status()
        raw = response.json()
        result = _parse_response(raw, started)
        state.history.extend([
            {"role": "user", "content": message},
            {"role": "assistant", "content": result.content},
        ])
        return result

    async def async_health(self, client: httpx.AsyncClient | None = None) -> dict[str, Any]:
        if client is not None:
            response = await client.get(f"{self.base_url}/health", timeout=10.0)
        else:
            async with httpx.AsyncClient() as ac:
                response = await ac.get(f"{self.base_url}/health", timeout=10.0)
        response.raise_for_status()
        return response.json()

    async def async_profiles(self, client: httpx.AsyncClient | None = None) -> dict[str, Any]:
        if client is not None:
            response = await client.get(f"{self.base_url}/profiles", timeout=10.0)
        else:
            async with httpx.AsyncClient() as ac:
                response = await ac.get(f"{self.base_url}/profiles", timeout=10.0)
        response.raise_for_status()
        return response.json()

    async def async_ingest(
        self,
        paths: list[str] | None = None,
        urls: list[str] | None = None,
        client: httpx.AsyncClient | None = None,
    ) -> dict[str, Any]:
        payload: dict[str, Any] = {"paths": paths or [], "urls": urls or []}
        if client is not None:
            response = await client.post(
                f"{self.base_url}/rag/ingest", json=payload, timeout=self.timeout
            )
        else:
            async with httpx.AsyncClient() as ac:
                response = await ac.post(
                    f"{self.base_url}/rag/ingest", json=payload, timeout=self.timeout
                )
        response.raise_for_status()
        return response.json()

    async def async_search(
        self, query: str, top_k: int = 5, client: httpx.AsyncClient | None = None
    ) -> dict[str, Any]:
        if client is not None:
            response = await client.post(
                f"{self.base_url}/rag/search",
                json={"query": query, "top_k": top_k},
                timeout=self.timeout,
            )
        else:
            async with httpx.AsyncClient() as ac:
                response = await ac.post(
                    f"{self.base_url}/rag/search",
                    json={"query": query, "top_k": top_k},
                    timeout=self.timeout,
                )
        response.raise_for_status()
        return response.json()

    async def async_list_rag_documents(
        self, client: httpx.AsyncClient | None = None
    ) -> dict[str, Any]:
        if client is not None:
            response = await client.get(f"{self.base_url}/rag/documents", timeout=10.0)
        else:
            async with httpx.AsyncClient() as ac:
                response = await ac.get(f"{self.base_url}/rag/documents", timeout=10.0)
        response.raise_for_status()
        return response.json()

    async def async_delete_rag_document(
        self, doc_id: str, client: httpx.AsyncClient | None = None
    ) -> dict[str, Any]:
        if client is not None:
            response = await client.delete(
                f"{self.base_url}/rag/documents/{doc_id}", timeout=10.0
            )
        else:
            async with httpx.AsyncClient() as ac:
                response = await ac.delete(
                    f"{self.base_url}/rag/documents/{doc_id}", timeout=10.0
                )
        response.raise_for_status()
        return response.json()

    async def async_compact(
        self, state: ChatState, client: httpx.AsyncClient | None = None
    ) -> ChatResult:
        """Compact conversation asynchronously (used by TUI)."""
        if not state.history:
            return ChatResult(
                content="No conversation history to compact.",
                wall_seconds=0,
                prompt_tps=None,
                generation_tps=None,
                raw={},
            )
        # Keep the most recent 30% of messages; summarize the older 70%
        split = max(1, len(state.history) - max(4, len(state.history) // 3))
        older = state.history[:split]
        recent = state.history[split:]

        transcript = "\n\n".join(
            f"{m['role']}: {m['content']}" for m in older
        )
        prompt = (
            "Compact the conversation below into a durable summary. "
            "Preserve user goals, decisions, constraints, important facts, open questions, "
            "file paths, commands, and unresolved work. Remove filler. Return only the summary.\n\n"
            f"{transcript}"
        )
        compact_state = ChatState(
            profile="complex_reasoning",
            quality="balanced",
            max_tokens=2048,
            skills_dir=state.skills_dir,
            context_tokens=state.context_tokens,
        )
        result = await self.async_send(prompt, compact_state, client=client)
        # Replace history: summary first, then retain recent raw messages
        state.history = [
            {"role": "system", "content": f"Conversation summary so far:\n{result.content}"},
            *recent,
        ]
        return result
