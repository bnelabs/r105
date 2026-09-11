"""Tests for tool execution, sandboxing, and expanded toolset."""

from __future__ import annotations

import json

from r105.tools import (
    TOOL_DEFINITIONS,
    calculate,
    convert,
    execute_python,
    execute_tool_call,
    get_time,
    system_info,
)


class TestToolDispatch:
    """Test that all tools dispatch correctly."""

    def test_tool_definitions_count(self):
        assert len(TOOL_DEFINITIONS) == 10

    def test_all_tools_have_required_fields(self):
        for tool in TOOL_DEFINITIONS:
            assert tool["type"] == "function"
            func = tool["function"]
            assert "name" in func
            assert "description" in func
            assert "parameters" in func

    def test_execute_python_dispatch(self, tmp_path):
        call = {
            "id": "1",
            "function": {
                "name": "execute_python",
                "arguments": '{"code": "print(42)"}',
            },
        }
        result = execute_tool_call(call, tmp_path)
        assert result["role"] == "tool"
        assert "42" in result["content"]

    def test_write_file_dispatch(self, tmp_path):
        call = {
            "id": "2",
            "function": {
                "name": "write_file",
                "arguments": '{"path": "test.txt", "content": "hello"}',
            },
        }
        result = execute_tool_call(call, tmp_path)
        assert "wrote" in result["content"] or "created" in result["content"]
        assert (tmp_path / "test.txt").read_text() == "hello"

    def test_read_file_dispatch(self, tmp_path):
        (tmp_path / "readme.txt").write_text("hello world")
        call = {
            "id": "3",
            "function": {
                "name": "read_file",
                "arguments": '{"path": "readme.txt"}',
            },
        }
        result = execute_tool_call(call, tmp_path)
        assert "hello world" in result["content"]
        assert "<tool_output>" in result["content"]

    def test_list_files_dispatch(self, tmp_path):
        (tmp_path / "a.txt").write_text("a")
        (tmp_path / "b.txt").write_text("b")
        call = {
            "id": "4",
            "function": {
                "name": "list_files",
                "arguments": '{"path": "."}',
            },
        }
        result = execute_tool_call(call, tmp_path)
        assert "a.txt" in result["content"]
        assert "b.txt" in result["content"]

    def test_unknown_tool(self, tmp_path):
        call = {
            "id": "99",
            "function": {"name": "nonexistent", "arguments": "{}"},
        }
        result = execute_tool_call(call, tmp_path)
        assert "unknown tool" in result["content"]


class TestSafeEval:
    """Test the safe expression evaluator for calculate()."""

    def test_simple_arithmetic(self):
        assert calculate({"expression": "2 + 3"}) == "5"
        assert calculate({"expression": "10 - 4"}) == "6"
        assert calculate({"expression": "3 * 7"}) == "21"
        assert calculate({"expression": "15 / 3"}) == "5.0"
        assert calculate({"expression": "2 ** 10"}) == "1024"

    def test_complex_expression(self):
        result = calculate({"expression": "2 + 3 * 4"})
        assert result == "14"

    def test_parentheses(self):
        result = calculate({"expression": "(2 + 3) * 4"})
        assert result == "20"

    def test_negative_numbers(self):
        assert calculate({"expression": "-5 + 3"}) == "-2"

    def test_floor_division(self):
        assert calculate({"expression": "7 // 2"}) == "3"

    def test_modulo(self):
        assert calculate({"expression": "10 % 3"}) == "1"

    def test_division_by_zero(self):
        result = calculate({"expression": "1 / 0"})
        assert "error" in result.lower()

    def test_rejects_unsafe_code(self):
        """The calculate() function must reject potentially dangerous expressions."""
        result = calculate({"expression": "__import__('os').system('ls')"})
        assert "error" in result.lower() or "unsafe" in result.lower()

    def test_rejects_function_calls(self):
        result = calculate({"expression": "eval('1+1')"})
        assert "error" in result.lower() or "unsafe" in result.lower()

    def test_empty_expression(self):
        result = calculate({"expression": ""})
        assert "error" in result.lower()

    def test_math_functions(self):
        assert calculate({"expression": "sqrt(16)"}) == "4.0"
        assert calculate({"expression": "2 * pi"}) == str(2 * 3.141592653589793)
        assert calculate({"expression": "abs(-7)"}) == "7"
        assert calculate({"expression": "round(2.5)"}) == "2"

    def test_unknown_name_rejected(self):
        result = calculate({"expression": "os.getcwd()"})
        assert "error" in result.lower() or "unsafe" in result.lower()

    def test_bool_constant_rejected(self):
        result = calculate({"expression": "True + 1"})
        assert "error" in result.lower() or "unsafe" in result.lower()


class TestConvertTool:
    """Tests for the convert unit-conversion tool."""

    def test_length(self):
        result = convert({"value": 1, "from_unit": "km", "to_unit": "m"})
        assert result == "1.0 km = 1000.0 m"

    def test_mass(self):
        result = convert({"value": 2.2, "from_unit": "lb", "to_unit": "kg"})
        assert "kg" in result
        assert "error" not in result.lower()

    def test_temperature(self):
        assert convert({"value": 0, "from_unit": "C", "to_unit": "F"}) == "0.0 C = 32.0 F"
        assert convert({"value": 32, "from_unit": "F", "to_unit": "C"}) == "32.0 F = 0.0 C"

    def test_time(self):
        assert convert({"value": 2, "from_unit": "h", "to_unit": "min"}) == "2.0 h = 120.0 min"

    def test_data(self):
        assert convert({"value": 1, "from_unit": "MiB", "to_unit": "KB"}) == (
            "1.0 MiB = 1048.576 KB"
        )

    def test_incompatible_units(self):
        result = convert({"value": 1, "from_unit": "km", "to_unit": "kg"})
        assert "error" in result.lower()

    def test_unknown_unit(self):
        result = convert({"value": 1, "from_unit": "furlong", "to_unit": "m"})
        assert "error" in result.lower()

    def test_non_numeric_value(self):
        result = convert({"value": "lots", "from_unit": "m", "to_unit": "km"})
        assert "error" in result.lower()

    def test_missing_units(self):
        assert "error" in convert({"value": 1, "from_unit": "", "to_unit": "m"}).lower()


class TestUtilityTools:
    """Tests for get_time and system_info."""

    def test_get_time_returns_iso_format(self):
        result = get_time()
        assert "T" in result  # ISO format has T separator
        # Should be parseable
        from datetime import datetime
        dt = datetime.fromisoformat(result)
        assert dt is not None

    def test_system_info_returns_json(self):
        result = system_info()
        info = json.loads(result)
        assert "platform" in info
        assert "python_version" in info
        assert "cpu_count" in info
        assert "hostname" in info
        assert isinstance(info["cpu_count"], int)


class TestSandboxedPython:
    """Tests for the sandboxed Python execution."""

    def test_basic_execution(self, tmp_path):
        result = execute_python({"code": "print('hello sandbox')"}, tmp_path)
        assert "hello sandbox" in result

    def test_stdout_captured(self, tmp_path):
        result = execute_python({"code": "for i in range(3): print(i)"}, tmp_path)
        assert "0" in result
        assert "1" in result
        assert "2" in result

    def test_stderr_captured(self, tmp_path):
        # Code that writes to stderr and exits with non-zero
        result = execute_python(
            {"code": "import sys; sys.stderr.write('error!'); sys.exit(1)"},
            tmp_path,
        )
        assert "error!" in result

    def test_exception_returns_stderr(self, tmp_path):
        result = execute_python({"code": "raise ValueError('test error')"}, tmp_path)
        assert "ValueError" in result

    def test_timeout(self, tmp_path):
        # The sandbox CPU limit (25s) or subprocess timeout (30s) will kill this
        result = execute_python({"code": "while True: pass"}, tmp_path)
        # Either the sandbox kills it (stderr) or the timeout catches it
        assert "timed out" in result.lower() or "killed" in result.lower() or "error" in result.lower()

    def test_isolated_filesystem(self, tmp_path):
        """Sandboxed code should not be able to write to the real workspace."""
        result = execute_python(
            {"code": "with open('/etc/shadow', 'w') as f: f.write('x')"},
            tmp_path,
        )
        # Should fail with permission error or file not found
        assert "error" in result.lower() or "Permission" in result or "denied" in result.lower()
