"""MCP (Model Context Protocol) client for connecting external tool servers.

Supports two transports:
- stdio: spawns a subprocess and communicates via stdin/stdout (JSON-RPC)
- sse: connects to an HTTP SSE endpoint (long-lived GET + POST for requests)

Configuration in config.json::

    {
      "mcp_servers": [
        {
          "name": "filesystem",
          "transport": "stdio",
          "command": "npx",
          "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
        },
        {
          "name": "github",
          "transport": "sse",
          "url": "http://127.0.0.1:3001/mcp"
        }
      ]
    }
"""

from __future__ import annotations

import abc
import json
import subprocess
import threading
from dataclasses import dataclass, field
from typing import Any

import httpx

from rova.errors import MCPConnectionError, MCPToolError


@dataclass
class MCPServerConfig:
    """Configuration for one MCP server connection.

    For stdio transport: set *command* (+ args, env).
    For sse transport: set *url* (SSE endpoint) + optionally *command* for
    local SSE servers that need a subprocess.
    """

    name: str
    command: str = ""
    args: list[str] = field(default_factory=list)
    env: dict[str, str] | None = None
    transport: str = "stdio"  # "stdio" or "sse"
    url: str = ""  # SSE endpoint, e.g. "http://127.0.0.1:3001/mcp"


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


# -- Abstract base ---------------------------------------------------------


class MCPClientBase(abc.ABC):
    """Abstract base for MCP transport clients."""

    def __init__(self, config: MCPServerConfig, timeout: float = 30.0) -> None:
        self._config = config
        self._timeout = timeout
        self._request_id = 0
        self._tools: dict[str, MCPTool] = {}
        self._connected = False

    @property
    @abc.abstractmethod
    def name(self) -> str:
        ...

    @property
    def tools(self) -> list[MCPTool]:
        return list(self._tools.values())

    @property
    def connected(self) -> bool:
        return self._connected

    # -- Lifecycle ----------------------------------------------------------

    @abc.abstractmethod
    def connect(self) -> None:
        """Connect to the MCP server and discover tools."""
        ...

    @abc.abstractmethod
    def close(self) -> None:
        """Disconnect from the MCP server."""
        ...

    # -- JSON-RPC -----------------------------------------------------------

    @abc.abstractmethod
    def _call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any] | None:
        """Send a JSON-RPC request and return the result."""
        ...

    def _init_and_discover(self) -> None:
        """Perform MCP initialization handshake and discover tools."""
        init_resp = self._call("initialize", {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "rova", "version": "0.2.1"},
        })
        if init_resp is None:
            raise MCPConnectionError(
                f"MCP server '{self._config.name}': no initialize response"
            )

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

    # -- Tool execution -----------------------------------------------------

    @abc.abstractmethod
    def call_tool(self, tool_name: str, arguments: dict[str, Any]) -> str:
        """Execute a tool via MCP and return the result as a string."""
        ...


# -- Stdio transport --------------------------------------------------------


class MCPStdioClient(MCPClientBase):
    """MCP client using stdio transport (subprocess stdin/stdout).

    Spawns a server process and communicates via JSON-RPC over pipes.
    """

    def __init__(self, config: MCPServerConfig, timeout: float = 30.0) -> None:
        super().__init__(config, timeout)
        self._proc: subprocess.Popen[bytes] | None = None
        self._lock = threading.Lock()

    @property
    def name(self) -> str:
        return self._config.name

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
            raise MCPConnectionError(
                f"MCP server '{self._config.name}': command not found: {self._config.command}"
            ) from err
        except OSError as exc:
            raise MCPConnectionError(
                f"MCP server '{self._config.name}': {exc}"
            ) from exc

        try:
            self._init_and_discover()
        except Exception:
            self.close()
            raise

    def close(self) -> None:
        """Terminate the MCP server subprocess."""
        self._connected = False
        self._tools.clear()
        if self._proc is not None:
            try:
                if self._proc.stdin:
                    self._proc.stdin.close()
                if self._proc.stdout:
                    self._proc.stdout.close()
                self._proc.terminate()
                self._proc.wait(timeout=5)
            except Exception:
                try:
                    self._proc.kill()
                except Exception:
                    pass
            finally:
                self._proc = None

    def _call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any] | None:
        """Send a JSON-RPC request via stdin and read response from stdout."""
        if self._proc is None or self._proc.stdin is None or self._proc.stdout is None:
            return None

        with self._lock:
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

    def call_tool(self, tool_name: str, arguments: dict[str, Any]) -> str:
        """Execute a tool via MCP and return the result as a string."""
        if not self._connected:
            raise MCPToolError(
                self._config.name, tool_name,
                f"MCP server '{self._config.name}' not connected",
            )

        result = self._call("tools/call", {
            "name": tool_name,
            "arguments": arguments,
        })

        if result is None:
            raise MCPToolError(
                self._config.name, tool_name,
                f"MCP tool '{tool_name}' returned no result",
            )

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


# -- SSE transport ----------------------------------------------------------


class MCPSSEClient(MCPClientBase):
    """MCP client using SSE (Server-Sent Events) transport.

    The MCP server exposes an SSE endpoint for receiving events and a separate
    endpoint (usually /message) for posting JSON-RPC requests.
    """

    def __init__(self, config: MCPServerConfig, timeout: float = 30.0) -> None:
        super().__init__(config, timeout)
        self._http: httpx.Client | None = None
        self._proc: subprocess.Popen[bytes] | None = None
        self._events_url: str = ""
        self._post_url: str = ""

    @property
    def name(self) -> str:
        return self._config.name

    def connect(self) -> None:
        """Connect to the SSE endpoint and discover tools."""
        if self._connected:
            return

        base_url = self._config.url.rstrip("/")
        self._events_url = base_url + "/events" if "/events" not in base_url else base_url
        self._post_url = base_url + "/message" if "/message" not in base_url else base_url

        try:
            self._http = httpx.Client(timeout=self._timeout)
            self._init_and_discover()
        except Exception:
            self.close()
            raise

    def close(self) -> None:
        """Disconnect from the MCP server."""
        self._connected = False
        self._tools.clear()
        if self._http is not None:
            try:
                self._http.close()
            except Exception:
                pass
            self._http = None
        if self._proc is not None:
            try:
                self._proc.terminate()
                self._proc.wait(timeout=5)
            except Exception:
                try:
                    self._proc.kill()
                except Exception:
                    pass
            self._proc = None

    def _call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any] | None:
        """Send a JSON-RPC request via HTTP POST and read response."""
        if self._http is None:
            return None

        self._request_id += 1
        request = {
            "jsonrpc": "2.0",
            "id": self._request_id,
            "method": method,
            "params": params or {},
        }

        try:
            response = self._http.post(
                self._post_url,
                json=request,
                timeout=self._timeout,
            )
            response.raise_for_status()
            data = response.json()
            if "error" in data:
                return None
            # POST may return empty for async SSE-based servers
            result = data.get("result")
            if result is not None:
                return result

            # If no result in POST response, the server may deliver it via
            # SSE stream later. For tool calls this is rare, but for
            # initialization the response should be synchronous.
            # Some SSE servers return 202 Accepted and deliver via stream.
            return self._poll_sse_result(self._request_id)
        except (httpx.HTTPError, json.JSONDecodeError):
            return None

    def _poll_sse_result(self, request_id: int, max_retries: int = 3) -> dict[str, Any] | None:
        """Poll the SSE stream for a response to a specific request ID."""
        for _ in range(max_retries):
            try:
                with self._http.stream("GET", self._events_url, timeout=self._timeout) as stream:
                    for line in stream.iter_lines():
                        if not line:
                            continue
                        if line.startswith("data: "):
                            data = json.loads(line[6:])
                            if data.get("id") == request_id:
                                return data.get("result")
                        elif line.startswith("event: "):
                            pass  # ignore event type for now
            except (httpx.HTTPError, json.JSONDecodeError, StopIteration):
                return None
        return None

    def call_tool(self, tool_name: str, arguments: dict[str, Any]) -> str:
        """Execute a tool via MCP SSE and return the result as a string."""
        if not self._connected:
            raise MCPToolError(
                self._config.name, tool_name,
                f"MCP server '{self._config.name}' not connected",
            )

        result = self._call("tools/call", {
            "name": tool_name,
            "arguments": arguments,
        })

        if result is None:
            raise MCPToolError(
                self._config.name, tool_name,
                f"MCP tool '{tool_name}' returned no result",
            )

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
    """Manages multiple MCP server connections (stdio + SSE)."""

    def __init__(self) -> None:
        self._clients: dict[str, MCPClientBase] = {}

    def connect_server(self, config: MCPServerConfig) -> str | None:
        """Connect to an MCP server. Returns None on success, error string on failure."""
        if config.name in self._clients:
            return f"server '{config.name}' already connected"

        if config.transport == "sse":
            if not config.url:
                return f"SSE server '{config.name}': missing 'url'"
            client = MCPSSEClient(config)
        else:
            if not config.command:
                return f"stdio server '{config.name}': missing 'command'"
            client = MCPStdioClient(config)

        try:
            client.connect()
        except MCPConnectionError as exc:
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

    def get_client(self, name: str) -> MCPClientBase | None:
        return self._clients.get(name)

    def list_servers(self) -> list[dict[str, Any]]:
        result: list[dict[str, Any]] = []
        for name, client in self._clients.items():
            result.append({
                "name": name,
                "connected": client.connected,
                "tool_count": len(client.tools),
                "transport": "sse" if isinstance(client, MCPSSEClient) else "stdio",
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
                    try:
                        return client.call_tool(tool.name, arguments)
                    except MCPToolError as exc:
                        return str(exc)
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
            transport=entry.get("transport", "stdio"),
            url=entry.get("url", ""),
        )
        if cfg.transport == "stdio" and not cfg.command:
            errors.append(f"MCP server '{cfg.name}': missing 'command' for stdio transport")
            continue
        if cfg.transport == "sse" and not cfg.url:
            errors.append(f"MCP server '{cfg.name}': missing 'url' for SSE transport")
            continue

        err = manager.connect_server(cfg)
        if err:
            errors.append(err)

    return errors
