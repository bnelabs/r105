"""Smoke tests for MCP client plumbing (config validation + manager).

No subprocesses are spawned and no network is touched: connection-failure
paths are exercised with configs that fail validation before dialing out.
"""

from r105.mcp_client import (
    MCPManager,
    MCPServerConfig,
    MCPTool,
    get_mcp_manager,
    load_mcp_servers,
)


class TestManagerValidation:
    def test_empty_manager_lists_nothing(self):
        manager = MCPManager()
        assert manager.list_servers() == []
        assert manager.get_all_tools() == []
        assert manager.get_all_definitions() == []
        assert manager.get_client("ghost") is None
        assert manager.disconnect_server("ghost") is False

    def test_stdio_without_command_rejected(self):
        manager = MCPManager()
        err = manager.connect_server(MCPServerConfig(name="bad"))
        assert err is not None and "command" in err
        assert manager.get_client("bad") is None

    def test_sse_without_url_rejected(self):
        manager = MCPManager()
        err = manager.connect_server(
            MCPServerConfig(name="bad", transport="sse")
        )
        assert err is not None and "url" in err

    def test_duplicate_name_rejected(self):
        manager = MCPManager()
        # Unconnectable command: second connect must fail on the duplicate
        # name, not attempt a second spawn.
        manager._clients["dup"] = _FakeClient()
        err = manager.connect_server(
            MCPServerConfig(name="dup", command="/bin/false")
        )
        assert err is not None and "already connected" in err
        manager.disconnect_all()

    def test_disconnect_all_clears(self):
        manager = MCPManager()
        manager._clients["a"] = _FakeClient()
        manager.disconnect_all()
        assert manager.list_servers() == []


class TestToolDefinition:
    def test_namespaced_definition_shape(self):
        tool = MCPTool(
            name="read",
            description="Read a file",
            parameters={"path": {"type": "string"}},
            required=["path"],
            server_name="fs",
        )
        defn = tool.to_definition()
        assert defn["function"]["name"] == "mcp_fs_read"
        assert defn["function"]["parameters"]["required"] == ["path"]
        assert defn["function"]["description"].startswith("[MCP:fs]")


class TestLoadServers:
    def test_invalid_entries_report_errors_without_connecting(self):
        errors = load_mcp_servers([
            {"name": "no-cmd", "transport": "stdio"},
            {"name": "no-url", "transport": "sse"},
        ])
        assert len(errors) == 2
        assert any("no-cmd" in e for e in errors)
        assert any("no-url" in e for e in errors)
        get_mcp_manager().disconnect_all()

    def test_empty_config_list_ok(self):
        assert load_mcp_servers([]) == []


class _FakeClient:
    connected = True
    tools: list = []

    def close(self):
        pass
