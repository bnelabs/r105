"""Tool-result cache boundedness tests (LRU eviction + explicit clear)."""

import json
from pathlib import Path

from r105.tools import (
    _TOOL_CACHE_MAX_SIZE,
    _cache_clear,
    _cached_result,
    _memoize_tool,
    _tool_cache,
    execute_tool_call,
)


def _read_call(path: str) -> dict:
    return {
        "id": "call_test_1",
        "function": {"name": "read_file", "arguments": json.dumps({"path": path})},
    }


class TestLruEviction:
    def setup_method(self):
        _cache_clear()

    def teardown_method(self):
        _cache_clear()

    def test_oldest_entries_evicted(self):
        for i in range(_TOOL_CACHE_MAX_SIZE + 10):
            _memoize_tool("calculate", f"{{\"expression\": \"{i}+1\"}}", str(i + 1))
        assert len(_tool_cache) == _TOOL_CACHE_MAX_SIZE
        # The first 10 inserted must be gone; the last one must be present.
        assert _cached_result("calculate", '{"expression": "0+1"}') is None
        assert _cached_result(
            "calculate", f'{{"expression": "{_TOOL_CACHE_MAX_SIZE + 9}+1"}}'
        ) is not None

    def test_cache_hit_refreshes_recency(self):
        for i in range(_TOOL_CACHE_MAX_SIZE):
            _memoize_tool("calculate", f"k{i}", str(i))
        assert _cached_result("calculate", "k0") == "0"  # touch oldest
        _memoize_tool("calculate", "new-key", "new")
        assert len(_tool_cache) == _TOOL_CACHE_MAX_SIZE
        assert _cached_result("calculate", "k0") == "0"  # survived
        assert _cached_result("calculate", "k1") is None  # evicted instead

    def test_clear_empties_cache(self):
        _memoize_tool("calculate", "k", "v")
        _cache_clear()
        assert _tool_cache == {}
        assert _cached_result("calculate", "k") is None


class TestExecuteToolCallMemoization:
    def setup_method(self):
        _cache_clear()

    def teardown_method(self):
        _cache_clear()

    def test_identical_calls_return_cached_note(self, tmp_path: Path):
        target = tmp_path / "note.txt"
        target.write_text("hello cache")
        first = execute_tool_call(_read_call("note.txt"), tmp_path)
        assert "hello cache" in first["content"]
        second = execute_tool_call(_read_call("note.txt"), tmp_path)
        assert "cached from a previous identical call" in second["content"]
        assert "hello cache" in second["content"]

    def test_use_cache_false_bypasses_memoization(self, tmp_path: Path):
        target = tmp_path / "note.txt"
        target.write_text("hello cache")
        execute_tool_call(_read_call("note.txt"), tmp_path)
        fresh = execute_tool_call(_read_call("note.txt"), tmp_path, use_cache=False)
        assert "cached from a previous identical call" not in fresh["content"]
