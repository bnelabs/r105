"""Central configuration constants for r105.

Replaces magic numbers scattered through the codebase with named
constants so defaults can be changed in one place.
"""

from __future__ import annotations

# -- Sandbox defaults ---------------------------------------------------

SANDBOX_MEMORY_MB: int = 256
SANDBOX_CPU_SECONDS: int = 25
SANDBOX_FILESIZE_MB: int = 50
SANDBOX_TIMEOUT: float = 30.0

# -- Per-tool timeouts (in seconds) --------------------------------------

TOOL_TIMEOUT_EXECUTE_PYTHON: float = 35.0   # sandbox + overhead
TOOL_TIMEOUT_WEB_SEARCH: float = 12.0
TOOL_TIMEOUT_WEB_FETCH: float = 15.0
TOOL_TIMEOUT_FILE_OPS: float = 10.0
TOOL_TIMEOUT_DEFAULT: float = 30.0

# -- Max output (in chars) before truncation ------------------------------

TOOL_MAX_OUTPUT_CHARS: int = 8000    # ~2000 BPE tokens

# -- Web tools ----------------------------------------------------------

WEB_SEARCH_TIMEOUT: float = 15.0
WEB_FETCH_TIMEOUT: float = 15.0
WEB_FETCH_MAX_CHARS: int = 8000
WEB_SEARCH_MAX_RESULTS: int = 10

# -- Chat / agent loop --------------------------------------------------

AUTO_COMPACT_THRESHOLD_PCT: int = 80
MAX_TOOL_LOOP_ITERATIONS: int = 10
MAX_REPEATED_TOOL_CALLS: int = 3
RECENT_CALL_TRACKING_SIZE: int = 6

# -- HTTP client --------------------------------------------------------

DEFAULT_HTTP_TIMEOUT: float = 300.0
COMPACT_RECENT_FRACTION: float = 0.30  # keep this fraction of most-recent messages
COMPACT_SUMMARY_TOKENS: int = 2048
COMPACT_PROFILE: str = "complex_reasoning"

# -- Compact prompt template (reused by sync + async methods) -----------

COMPACT_PROMPT_TEMPLATE: str = (
    "Compact the conversation below into a durable summary for continuing the same chat. "
    "Preserve user goals, decisions, constraints, important facts, open questions, file paths, "
    "commands, and unresolved work. Remove filler and repeated wording. Return only the summary.\n\n"
    "{transcript}"
)

# -- Session workspace --------------------------------------------------

SESSION_DATE_FORMAT: str = "%Y-%m-%d"
