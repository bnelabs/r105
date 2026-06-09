"""HTTP client for llama-router API."""

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
        text = read_skill(state.skills_dir, name)
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


class RouterClient:
    """HTTP client for the llama-router API."""

    def __init__(
        self,
        base_url: str = "http://127.0.0.1:8010",
        timeout: float = 300.0,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self.timeout = timeout

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

    def send(self, message: str, state: ChatState, tools: list[dict[str, Any]] | None = None) -> ChatResult:
        """Send a message and return the result. If the response contains tool
        calls they are included in the result but NOT auto-executed — the
        caller is responsible for the tool loop."""
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

        started = time.perf_counter()
        response = httpx.post(
            f"{self.base_url}/v1/chat/completions",
            json=payload,
            timeout=self.timeout,
        )
        response.raise_for_status()
        raw = response.json()
        wall_seconds = time.perf_counter() - started
        content = _extract_assistant_content(raw)
        tool_calls = _extract_tool_calls(raw)
        timings = raw.get("timings") or {}
        state.history.extend(
            [
                {"role": "user", "content": message},
                {"role": "assistant", "content": content},
            ]
        )
        return ChatResult(
            content=content,
            wall_seconds=wall_seconds,
            prompt_tps=_maybe_float(timings.get("prompt_per_second")),
            generation_tps=_maybe_float(timings.get("predicted_per_second")),
            raw=raw,
            tool_calls=tool_calls,
        )

    def compact(self, state: ChatState) -> ChatResult:
        """Summarize conversation history and replace it with the summary."""
        if not state.history:
            return ChatResult(
                content="No conversation history to compact.",
                wall_seconds=0,
                prompt_tps=None,
                generation_tps=None,
                raw={},
            )
        transcript = "\n\n".join(
            f"{message['role']}: {message['content']}" for message in state.history
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
        state.history = [
            {"role": "system", "content": f"Conversation summary so far:\n{result.content}"}
        ]
        return result
