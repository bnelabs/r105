"""Tool-loop mechanics tests (pure logic extracted to r105.tool_loop)."""

import json

from r105.constants import (
    MAX_REPEATED_TOOL_CALLS,
    TOOL_TIMEOUT_DEFAULT,
    TOOL_TIMEOUT_EXECUTE_PYTHON,
    TOOL_TIMEOUT_FILE_OPS,
    TOOL_TIMEOUT_WEB_FETCH,
    TOOL_TIMEOUT_WEB_SEARCH,
)
from r105.tool_loop import (
    LoopDedupTracker,
    exception_result,
    parse_tool_signatures,
    timeout_result,
    tool_timeout,
)


def _tc(name: str, arguments) -> dict:
    return {"id": "call_1", "function": {"name": name, "arguments": arguments}}


class TestParseToolSignatures:
    def test_json_string_arguments(self):
        sigs = parse_tool_signatures([_tc("read_file", '{"path": "a.txt"}')])
        assert sigs[0][1] == "read_file"
        assert json.loads(sigs[0][2]) == {"path": "a.txt"}

    def test_dict_arguments(self):
        sigs = parse_tool_signatures([_tc("calculate", {"expression": "1+1"})])
        assert json.loads(sigs[0][2]) == {"expression": "1+1"}

    def test_malformed_arguments_fall_back_to_empty(self):
        sigs = parse_tool_signatures([_tc("calculate", "{oops")])
        _, name, args_str = sigs[0]
        assert name == "calculate"
        assert json.loads(args_str) == {}

    def test_missing_function_defaults(self):
        sigs = parse_tool_signatures([{"id": "x"}])
        assert sigs[0][1] == "unknown"


class TestLoopDedupTracker:
    def test_first_call_neither_stuck_nor_repeat(self):
        tracker = LoopDedupTracker()
        assert tracker.check("read_file", "{}") == (False, False)

    def test_second_identical_call_is_repeat_not_stuck(self):
        tracker = LoopDedupTracker()
        tracker.check("read_file", "{}")
        assert tracker.check("read_file", "{}") == (False, True)

    def test_stuck_after_max_repeats(self):
        tracker = LoopDedupTracker()
        stuck = False
        for _ in range(MAX_REPEATED_TOOL_CALLS + 1):
            stuck, _ = tracker.check("get_time", "{}")
        assert stuck is True

    def test_different_args_do_not_trigger(self):
        tracker = LoopDedupTracker()
        tracker.check("calculate", '{"expression": "1+1"}')
        assert tracker.check("calculate", '{"expression": "2+2"}') == (False, False)

    def test_duplicate_result_flag(self):
        tracker = LoopDedupTracker()
        tracker.check("read_file", "{}")
        assert tracker.is_duplicate_result("read_file", "{}") is False
        tracker.check("read_file", "{}")
        assert tracker.is_duplicate_result("read_file", "{}") is True

    def test_stop_loop_message_names_tool(self):
        msg = LoopDedupTracker.stop_loop_message("read_file", 4)
        assert "read_file" in msg and "4" in msg and "STOP LOOP" in msg


class TestTimeouts:
    def test_per_tool_timeouts(self):
        assert tool_timeout("execute_python") == TOOL_TIMEOUT_EXECUTE_PYTHON
        assert tool_timeout("web_search") == TOOL_TIMEOUT_WEB_SEARCH
        assert tool_timeout("web_fetch") == TOOL_TIMEOUT_WEB_FETCH
        assert tool_timeout("read_file") == TOOL_TIMEOUT_FILE_OPS
        assert tool_timeout("write_file") == TOOL_TIMEOUT_FILE_OPS
        assert tool_timeout("list_files") == TOOL_TIMEOUT_FILE_OPS
        assert tool_timeout("something_else") == TOOL_TIMEOUT_DEFAULT

    def test_timeout_result_shape(self):
        tc = _tc("web_fetch", "{}")
        result = timeout_result(tc, "web_fetch", 30.0)
        assert result["role"] == "tool"
        assert result["tool_call_id"] == "call_1"
        assert "timed out after 30.0s" in result["content"]

    def test_exception_result_shape(self):
        tc = _tc("calculate", "{}")
        result = exception_result(tc, "calculate", ValueError("boom"))
        assert result["content"] == "error: boom"
        assert result["name"] == "calculate"
