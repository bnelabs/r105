"""Integration tests for RouterClient using mocked HTTP responses."""

from __future__ import annotations

import asyncio
import json
import time
from pathlib import Path

import httpx
import pytest
from pytest_httpx import HTTPXMock

from rova.client import RouterClient
from rova.state import ChatState


def _ok_response(content: str = "OK", tool_calls: list | None = None) -> dict:
    msg: dict = {"role": "assistant", "content": content}
    if tool_calls:
        msg["tool_calls"] = tool_calls
    return {
        "choices": [{"message": msg, "finish_reason": "stop"}],
        "timings": {"prompt_per_second": 100.0, "predicted_per_second": 50.0},
    }


def _sse_lines(*chunks: str) -> str:
    """Build an SSE stream from content chunks."""
    lines: list[str] = []
    for i, chunk in enumerate(chunks):
        data = json.dumps({
            "choices": [{
                "index": 0,
                "delta": {"content": chunk, "tool_calls": None},
                "finish_reason": "stop" if i == len(chunks) - 1 else None,
            }]
        })
        lines.append(f"data: {data}\n")
    lines.append("data: [DONE]\n")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# async_send
# ---------------------------------------------------------------------------

class TestAsyncSend:
    async def test_sends_message_and_records_history(
        self, httpx_mock: HTTPXMock, router_client: RouterClient, chat_state: ChatState
    ) -> None:
        httpx_mock.add_response(
            url="http://127.0.0.1:8010/v1/chat/completions",
            method="POST",
            json=_ok_response(content="I am an assistant."),
        )

        async with httpx.AsyncClient() as client:
            result = await router_client.async_send("Hello", chat_state, client=client)

        assert result.content == "I am an assistant."
        assert result.wall_seconds > 0
        assert len(chat_state.history) == 2
        assert chat_state.history[0] == {"role": "user", "content": "Hello"}
        assert chat_state.history[1] == {"role": "assistant", "content": "I am an assistant."}

    async def test_sends_tool_calls_in_history(
        self, httpx_mock: HTTPXMock, router_client: RouterClient, chat_state: ChatState
    ) -> None:
        tool_calls = [{
            "id": "call_1",
            "type": "function",
            "function": {"name": "web_search", "arguments": '{"query":"test"}'},
        }]
        httpx_mock.add_response(
            url="http://127.0.0.1:8010/v1/chat/completions",
            method="POST",
            json=_ok_response(content="Searching...", tool_calls=tool_calls),
        )

        async with httpx.AsyncClient() as client:
            result = await router_client.async_send("search test", chat_state, client=client)

        assert result.tool_calls == tool_calls
        assistant_msg = chat_state.history[1]
        assert assistant_msg["role"] == "assistant"
        assert assistant_msg["content"] == "Searching..."
        assert assistant_msg["tool_calls"] == tool_calls

    async def test_http_error_propagates(
        self, httpx_mock: HTTPXMock, router_client: RouterClient, chat_state: ChatState
    ) -> None:
        httpx_mock.add_response(
            url="http://127.0.0.1:8010/v1/chat/completions",
            method="POST",
            status_code=500,
        )

        async with httpx.AsyncClient() as client:
            with pytest.raises(httpx.HTTPStatusError):
                await router_client.async_send("hello", chat_state, client=client)


# ---------------------------------------------------------------------------
# async_continue
# ---------------------------------------------------------------------------

class TestAsyncContinue:
    async def test_continues_without_user_message(
        self, httpx_mock: HTTPXMock, router_client: RouterClient, chat_state_with_history: ChatState
    ) -> None:
        """async_continue must NOT inject a user message — it sends history as-is."""
        history_before = len(chat_state_with_history.history)

        httpx_mock.add_response(
            url="http://127.0.0.1:8010/v1/chat/completions",
            method="POST",
            json=_ok_response(content="Here are the results of the search."),
        )

        async with httpx.AsyncClient() as client:
            result = await router_client.async_continue(
                chat_state_with_history, client=client
            )

        assert result.content == "Here are the results of the search."
        # Only the assistant response was appended (no user message)
        assert len(chat_state_with_history.history) == history_before + 1
        last = chat_state_with_history.history[-1]
        assert last["role"] == "assistant"

    async def test_continue_streaming(
        self, httpx_mock: HTTPXMock, router_client: RouterClient, chat_state_with_history: ChatState
    ) -> None:
        """async_continue with on_chunk uses SSE streaming."""
        history_before = len(chat_state_with_history.history)

        httpx_mock.add_response(
            url="http://127.0.0.1:8010/v1/chat/completions",
            method="POST",
            text=_sse_lines("Here", " are", " the results."),
        )

        chunks: list[str] = []

        async with httpx.AsyncClient() as client:
            result = await router_client.async_continue(
                chat_state_with_history,
                client=client,
                on_chunk=lambda token: chunks.append(token),
            )

        assert result.content == "Here are the results."
        assert chunks == ["Here", " are", " the results."]
        assert len(chat_state_with_history.history) == history_before + 1


# ---------------------------------------------------------------------------
# async_send_streaming (SSE)
# ---------------------------------------------------------------------------

class TestAsyncSendStreaming:
    async def test_streams_content_incrementally(
        self, httpx_mock: HTTPXMock, router_client: RouterClient, chat_state: ChatState
    ) -> None:
        httpx_mock.add_response(
            url="http://127.0.0.1:8010/v1/chat/completions",
            method="POST",
            text=_sse_lines("Hello", " world", "!"),
        )

        chunks: list[str] = []

        async with httpx.AsyncClient() as client:
            result = await router_client.async_send_streaming(
                "Hi!", chat_state, client=client, on_chunk=lambda t: chunks.append(t)
            )

        assert result.content == "Hello world!"
        assert chunks == ["Hello", " world", "!"]
        assert len(chat_state.history) == 2  # user + assistant

    async def test_streaming_with_tool_calls(
        self, httpx_mock: HTTPXMock, router_client: RouterClient, chat_state: ChatState
    ) -> None:
        """Tool calls arrive as deltas across SSE chunks."""
        sse = (
            'data: {"choices":[{"index":0,"delta":{"content":"Let me search.","tool_calls":null},"finish_reason":null}]}\n'
            'data: {"choices":[{"index":0,"delta":{"content":"","tool_calls":[{"index":0,"id":"t1","type":"function","function":{"name":"web","arguments":""}}]},"finish_reason":null}]}\n'
            'data: {"choices":[{"index":0,"delta":{"content":"","tool_calls":[{"index":0,"id":"","type":"","function":{"name":"","arguments":"{\\"q\\":\\"x\\"}"}}]},"finish_reason":"tool_calls"}]}\n'
            "data: [DONE]\n"
        )
        httpx_mock.add_response(
            url="http://127.0.0.1:8010/v1/chat/completions",
            method="POST",
            text=sse,
        )

        async with httpx.AsyncClient() as client:
            result = await router_client.async_send_streaming(
                "search for x", chat_state, client=client
            )

        assert len(result.tool_calls) == 1
        assert result.tool_calls[0]["id"] == "t1"
        assert result.tool_calls[0]["function"]["name"] == "web"
        assert "q" in result.tool_calls[0]["function"]["arguments"]


# ---------------------------------------------------------------------------
# async_compact
# ---------------------------------------------------------------------------

class TestAsyncCompact:
    async def test_compact_empty_history(
        self, router_client: RouterClient, chat_state: ChatState
    ) -> None:
        """Compacting empty history returns placeholder without HTTP calls."""
        result = await router_client.async_compact(chat_state)
        assert result.content == "No conversation history to compact."
        assert result.wall_seconds == 0

    async def test_compact_summarizes_history(
        self, httpx_mock: HTTPXMock, router_client: RouterClient, chat_state: ChatState
    ) -> None:
        """Compact replaces older history with a summary."""
        # Populate enough history to trigger a meaningful split
        for i in range(10):
            chat_state.history.append({"role": "user", "content": f"message {i}"})
            chat_state.history.append({"role": "assistant", "content": f"reply {i}"})

        original_len = len(chat_state.history)

        httpx_mock.add_response(
            url="http://127.0.0.1:8010/v1/chat/completions",
            method="POST",
            json=_ok_response(content="Summarized conversation."),
        )

        async with httpx.AsyncClient() as client:
            result = await router_client.async_compact(chat_state, client=client)

        assert result.content == "Summarized conversation."
        # History is now shorter: 1 system summary + ~30% recent messages
        assert len(chat_state.history) < original_len
        assert chat_state.history[0]["role"] == "system"
        assert "Summarized conversation" in chat_state.history[0]["content"]


# ---------------------------------------------------------------------------
# Health & Profiles
# ---------------------------------------------------------------------------

class TestAsyncHealth:
    async def test_health_returns_dict(
        self, httpx_mock: HTTPXMock, router_client: RouterClient
    ) -> None:
        httpx_mock.add_response(
            url="http://127.0.0.1:8010/health",
            method="GET",
            json={"status": "ok", "model": "gemma-4-12b-it"},
        )

        async with httpx.AsyncClient() as client:
            result = await router_client.async_health(client=client)

        assert result["status"] == "ok"
        assert result["model"] == "gemma-4-12b-it"


class TestAsyncProfiles:
    async def test_profiles_returns_dict(
        self, httpx_mock: HTTPXMock, router_client: RouterClient
    ) -> None:
        httpx_mock.add_response(
            url="http://127.0.0.1:8010/profiles",
            method="GET",
            json={"profiles": {"coding": {"max_tokens": 4096}}},
        )

        async with httpx.AsyncClient() as client:
            result = await router_client.async_profiles(client=client)

        assert "profiles" in result
        assert "coding" in result["profiles"]


# ---------------------------------------------------------------------------
# RAG operations
# ---------------------------------------------------------------------------

class TestAsyncRag:
    async def test_ingest(
        self, httpx_mock: HTTPXMock, router_client: RouterClient
    ) -> None:
        httpx_mock.add_response(
            url="http://127.0.0.1:8010/rag/ingest",
            method="POST",
            json={"files_indexed": 3, "chunks_indexed": 12},
        )

        async with httpx.AsyncClient() as client:
            result = await router_client.async_ingest(
                paths=["/tmp/doc.txt"], client=client
            )

        assert result["files_indexed"] == 3
        assert result["chunks_indexed"] == 12

    async def test_search(
        self, httpx_mock: HTTPXMock, router_client: RouterClient
    ) -> None:
        httpx_mock.add_response(
            url="http://127.0.0.1:8010/rag/search",
            method="POST",
            json={"results": [{"source_tag": "doc.txt", "score": 0.95, "text": "content here"}]},
        )

        async with httpx.AsyncClient() as client:
            result = await router_client.async_search("query", client=client)

        assert len(result["results"]) == 1

    async def test_list_documents(
        self, httpx_mock: HTTPXMock, router_client: RouterClient
    ) -> None:
        httpx_mock.add_response(
            url="http://127.0.0.1:8010/rag/documents",
            method="GET",
            json={"documents": [{"id": "d1", "source": "doc.txt", "chunks": 5}]},
        )

        async with httpx.AsyncClient() as client:
            result = await router_client.async_list_rag_documents(client=client)

        assert len(result["documents"]) == 1

    async def test_delete_document(
        self, httpx_mock: HTTPXMock, router_client: RouterClient
    ) -> None:
        httpx_mock.add_response(
            url="http://127.0.0.1:8010/rag/documents/d1",
            method="DELETE",
            json={"deleted": 1},
        )

        async with httpx.AsyncClient() as client:
            result = await router_client.async_delete_rag_document("d1", client=client)

        assert result["deleted"] == 1
