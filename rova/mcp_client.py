"""MCP (Model Context Protocol) client for connecting external tool servers.

MCP servers expose tools via JSON-RPC over stdio. This module provides
an MCPClient that connects to servers, discovers tools, and executes them.

Configuration in config.json::

    {
      "mcp_servers": [
        {
          "name": "filesystem",
          "command": "npx",
          "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
        }
      ]
    }
"""

from __future__ import annotations

import json
import subprocess
from dataclasses import dataclass, field
from typing import Any


@dataclass
class MCPServerConfig:
    """Configuration for one MCP server connection."""

    name: str
    command: str
    args: list[str] = field(default_factory=list)
    env: dict[str, str] | None = None


@dataclass
class MCPTool:
    """A tool discovered from an MCP server."""

    name: str
    description: str = ""
    parameters: dict[str, Any] = field(default_factory=dict)
    required: list[str] = field(default_factory=list)
    server_name: str = ""

    def to_definition(self) -> dict[str, Any]:
        """Convert to standard TOOL_DEFINITIONS format."""
        schema: dict[str, Any] = {
            "type": "object",
            "properties": self.parameters,
        }
        if self.required:
            schema["required"] = self.required

        return {
            "type": "function",
            "function": {
                "name": f"mcp_{self.server_name}_{self.name}",
                "description": f"[MCP:{self.server_name}] {self.description}",
                "parameters": schema,
            },
        }


class MCPClient:
    """Client for a single MCP server process.

    Communicates via JSON-RPC over stdin/stdout. Manages the subprocess
    lifecycle and provides tool discovery + execution.
    """

    def __init__(self, config: MCPServerConfig, timeout: float = 30.0) -> None:
        self._config = config
        self._timeout = timeout
        self._proc: subprocess.Popen[bytes] | None = None
        self._request_id = 0
        self._tools: dict[str, MCPTool] = {}
        self._connected = False

    @property
    def name(self) -> str:
        return self._config.name

    @property
    def tools(self) -> list[MCPTool]:
        return list(self._tools.values())

    @property
    def connected(self) -> bool:
        return self._connected

    # -- Lifecycle ----------------------------------------------------------

    def connect(self) -> None:
        """Start the MCP server subprocess and perform initialization."""
        if self._connected:
            return

        try:
            self._proc = subprocess.Popen(
                [self._config.command, *self._config.args],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                env=self._config.env,
            )
        except FileNotFoundError as err:
            raise ConnectionError(
                f"MCP server '{self._config.name}': command not found: {self._config.command}"
            ) from err
        except OSError as exc:
            raise ConnectionError(
                f"MCP server '{self._config.name}': {exc}"
            ) from exc

        # Initialize handshake
        try:
            init_resp = self._call("initialize", {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "rova", "version": "0.2.0"},
            })
            if init_resp is None:
                raise ConnectionError(f"MCP server '{self._config.name}': no initialize response")

            # Discover tools
            tools_resp = self._call("tools/list")
            if tools_resp and "tools" in tools_resp:
                for tool_data in tools_resp["tools"]:
                    schema = tool_data.get("inputSchema") or tool_data.get("parameters") or {}
                    tool = MCPTool(
                        name=tool_data.get("name", "unknown"),
                        description=tool_data.get("description", ""),
                        parameters=schema.get("properties", {}),
                        required=schema.get("required", []),
                        server_name=self._config.name,
                    )
                    self._tools[tool.name] = tool

            self._connected = True
        except Exception:
            self.close()
            raise

    def close(self) -> None:
        """Terminate the MCP server subprocess."""
        self._connected = False
        self._tools.clear()
        if self._proc is not None:
            try:
                self._proc.stdin.close()  # type: ignore[union-attr]
                self._proc.stdout.close()  # type: ignore[union-attr]
                self._proc.terminate()
                self._proc.wait(timeout=5)
            except Exception:
                try:
                    self._proc.kill()
                except Exception:
                    pass
            finally:
                self._proc = None

    # -- JSON-RPC -----------------------------------------------------------

    def _call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any] | None:
        """Send a JSON-RPC request and return the result."""
        if self._proc is None or self._proc.stdin is None or self._proc.stdout is None:
            return None

        self._request_id += 1
        request = {
            "jsonrpc": "2.0",
            "id": self._request_id,
            "method": method,
            "params": params or {},
        }

        try:
            payload = json.dumps(request, ensure_ascii=False) + "\n"
            self._proc.stdin.write(payload.encode("utf-8"))
            self._proc.stdin.flush()

            # Read response line
            line = self._proc.stdout.readline()
            if not line:
                return None

            response = json.loads(line.decode("utf-8"))
            if "error" in response:
                return None
            return response.get("result")
        except (OSError, json.JSONDecodeError, UnicodeDecodeError):
            return None

    # -- Tool execution -----------------------------------------------------

    def call_tool(self, tool_name: str, arguments: dict[str, Any]) -> str:
        """Execute a tool via MCP and return the result as a string."""
        if not self._connected:
            return f"error: MCP server '{self._config.name}' not connected"

        result = self._call("tools/call", {
            "name": tool_name,
            "arguments": arguments,
        })

        if result is None:
            return f"error: MCP tool '{tool_name}' returned no result"

        # MCP returns content as a list of content items
        content = result.get("content") or []
        if isinstance(content, list):
            parts: list[str] = []
            for item in content:
                if isinstance(item, dict):
                    text = item.get("text", "")
                    if isinstance(text, str):
                        parts.append(text)
            return "\n".join(parts) if parts else json.dumps(result, sort_keys=True)

        return json.dumps(result, sort_keys=True)


# -- Multi-server manager -------------------------------------------------


class MCPManager:
    """Manages multiple MCP server connections."""

    def __init__(self) -> None:
        self._clients: dict[str, MCPClient] = {}

    def connect_server(self, config: MCPServerConfig) -> str | None:
        """Connect to an MCP server. Returns None on success, error string on failure."""
        if config.name in self._clients:
            return f"server '{config.name}' already connected"

        client = MCPClient(config)
        try:
            client.connect()
        except ConnectionError as exc:
            return str(exc)
        except Exception as exc:
            return f"failed to connect to '{config.name}': {exc}"

        self._clients[config.name] = client
        return None  # success

    def disconnect_server(self, name: str) -> bool:
        """Disconnect and remove an MCP server. Returns True if it existed."""
        client = self._clients.pop(name, None)
        if client:
            client.close()
            return True
        return False

    def disconnect_all(self) -> None:
        for client in self._clients.values():
            client.close()
        self._clients.clear()

    def get_client(self, name: str) -> MCPClient | None:
        return self._clients.get(name)

    def list_servers(self) -> list[dict[str, Any]]:
        result: list[dict[str, Any]] = []
        for name, client in self._clients.items():
            result.append({
                "name": name,
                "connected": client.connected,
                "tool_count": len(client.tools),
            })
        return result

    def get_all_tools(self) -> list[MCPTool]:
        tools: list[MCPTool] = []
        for client in self._clients.values():
            tools.extend(client.tools)
        return tools

    def get_all_definitions(self) -> list[dict[str, Any]]:
        return [t.to_definition() for t in self.get_all_tools()]

    def execute_tool(self, full_name: str, arguments: dict[str, Any]) -> str | None:
        """Execute an MCP tool by its full name (mcp_<server>_<tool>).

        Returns the result string, or None if the tool is not found.
        """
        for client in self._clients.values():
            for tool in client.tools:
                fq_name = f"mcp_{tool.server_name}_{tool.name}"
                if fq_name == full_name:
                    return client.call_tool(tool.name, arguments)
        return None


# -- Module-level singleton ------------------------------------------------

_manager: MCPManager | None = None


def get_mcp_manager() -> MCPManager:
    global _manager
    if _manager is None:
        _manager = MCPManager()
    return _manager


def load_mcp_servers(configs: list[dict[str, Any]]) -> list[str]:
    """Load MCP server configs from config.json format.

    Returns a list of error messages (empty = all good).
    """
    manager = get_mcp_manager()
    errors: list[str] = []

    for entry in configs:
        cfg = MCPServerConfig(
            name=entry.get("name", "unnamed"),
            command=entry.get("command", ""),
            args=entry.get("args") or [],
            env=entry.get("env"),
        )
        if not cfg.command:
            errors.append(f"MCP server '{cfg.name}': missing 'command'")
            continue

        err = manager.connect_server(cfg)
        if err:
            errors.append(err)

    return errors
