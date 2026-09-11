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

    def test_reconnect_reuses_config_and_replaces_client(self, monkeypatch):
        manager = MCPManager()
        config = MCPServerConfig(name="reconnect", command="server")
        old = _ReconnectClient(config)
        manager._clients[config.name] = old
        created: list[_ReconnectClient] = []

        def make_client(cfg):
            client = _ReconnectClient(cfg)
            created.append(client)
            return client

        monkeypatch.setattr(manager, "_make_client", make_client)
        assert manager.reconnect_server("reconnect") is None
        assert old.closed is True
        assert len(created) == 1
        assert manager.get_client("reconnect") is created[0]
        assert created[0].connected is True

    def test_failed_reconnect_removes_stale_client(self, monkeypatch):
        manager = MCPManager()
        config = MCPServerConfig(name="broken", command="server")
        manager._clients[config.name] = _ReconnectClient(config)

        def make_client(cfg):
            return _ReconnectClient(cfg, connect_error="server unavailable")

        monkeypatch.setattr(manager, "_make_client", make_client)
        error = manager.reconnect_server("broken")
        assert error is not None and "server unavailable" in error
        assert manager.get_client("broken") is None

    def test_async_reconnect_reuses_config(self, monkeypatch):
        manager = MCPManager()
        config = MCPServerConfig(name="async-reconnect", command="server")
        old = _ReconnectClient(config)
        manager._clients[config.name] = old
        created: list[_ReconnectClient] = []

        def make_client(cfg):
            client = _ReconnectClient(cfg)
            created.append(client)
            return client

        monkeypatch.setattr(manager, "_make_client", make_client)

        import asyncio

        assert asyncio.run(manager.async_reconnect_server(config.name)) is None
        assert old.async_closed is True
        assert manager.get_client(config.name) is created[0]

    def test_reconnect_command_reports_success(self, monkeypatch):
        from r105.commands import _handle_mcp_command

        manager = MCPManager()
        config = MCPServerConfig(name="command-reconnect", command="server")
        manager._clients[config.name] = _ReconnectClient(config)
        monkeypatch.setattr("r105.commands.get_mcp_manager", lambda: manager)
        monkeypatch.setattr(manager, "_make_client", lambda cfg: _ReconnectClient(cfg))

        result = _handle_mcp_command(["reconnect", config.name])
        assert "reconnected" in result


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


class _ReconnectClient:
    def __init__(self, config, connect_error=None):
        self._config = config
        self.connected = True
        self.tools = []
        self.closed = False
        self.async_closed = False
        self._connect_error = connect_error

    @property
    def config(self):
        return self._config

    def close(self):
        self.closed = True
        self.connected = False

    def connect(self):
        if self._connect_error:
            from r105.errors import MCPConnectionError

            raise MCPConnectionError(self._connect_error)
        self.connected = True

    async def async_close(self):
        self.async_closed = True
        self.connected = False

    async def async_connect(self):
        self.connected = True
