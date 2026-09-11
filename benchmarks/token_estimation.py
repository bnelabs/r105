"""Compare r105's local token estimates with tiktoken on sample prompts.

Run from the repository root with::

    python benchmarks/token_estimation.py

The benchmark is diagnostic rather than a correctness gate: llama.cpp and
other SentencePiece backends can legitimately differ from tiktoken.
"""

from __future__ import annotations

import statistics
import sys
from dataclasses import dataclass

from r105.state import estimate_tokens


@dataclass(frozen=True)
class Sample:
    name: str
    text: str


SAMPLES = (
    Sample("english", "Explain how a database index reduces lookup time."),
    Sample("code", "def fib(n):\n    return n if n < 2 else fib(n - 1) + fib(n - 2)"),
    Sample("unicode", "Kısa bir Türkçe açıklama ve 日本語の例です。"),
    Sample("structured", '{"role":"user","content":"Summarize this request."}'),
)


def compare_samples() -> list[dict[str, float | int | str]]:
    """Return estimate/error rows using tiktoken's cl100k baseline."""
    try:
        import tiktoken
    except ImportError as exc:  # pragma: no cover - exercised by CLI use
        raise RuntimeError("install tiktoken with `pip install 'r105[dev]'`") from exc

    encoding = tiktoken.get_encoding("cl100k_base")
    rows: list[dict[str, float | int | str]] = []
    for sample in SAMPLES:
        actual = len(encoding.encode(sample.text))
        estimated = estimate_tokens(sample.text)
        error_pct = abs(estimated - actual) / actual * 100 if actual else 0.0
        rows.append(
            {
                "name": sample.name,
                "actual": actual,
                "estimated": estimated,
                "error_pct": error_pct,
            }
        )
    return rows


def main() -> int:
    try:
        rows = compare_samples()
    except RuntimeError as exc:
        print(exc, file=sys.stderr)
        return 2
    for row in rows:
        print(
            f"{row['name']:10} actual={row['actual']:4} "
            f"estimated={row['estimated']:4} error={row['error_pct']:.1f}%"
        )
    mean_error = statistics.mean(float(row["error_pct"]) for row in rows)
    print(f"mean absolute error: {mean_error:.1f}%")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
