"""Math helpers for tools — safe arithmetic evaluation.

Extracted from ``r105/tools.py``. No registry imports.
"""

from __future__ import annotations

import ast
import operator
from typing import Any

_SAFE_OPS: dict[type, Any] = {
    ast.Add: operator.add,
    ast.Sub: operator.sub,
    ast.Mult: operator.mul,
    ast.Div: operator.truediv,
    ast.FloorDiv: operator.floordiv,
    ast.Mod: operator.mod,
    ast.Pow: operator.pow,
    ast.USub: operator.neg,
    ast.UAdd: operator.pos,
}


def safe_eval(node: ast.AST) -> Any:
    """Recursively evaluate a safe AST expression (no builtins, no calls)."""
    if isinstance(node, ast.Constant):
        return node.value
    if isinstance(node, ast.UnaryOp):
        op = _SAFE_OPS.get(type(node.op))
        if op is None:
            raise ValueError(f"unsafe operator: {type(node.op).__name__}")
        return op(safe_eval(node.operand))
    if isinstance(node, ast.BinOp):
        op = _SAFE_OPS.get(type(node.op))
        if op is None:
            raise ValueError(f"unsafe operator: {type(node.op).__name__}")
        return op(safe_eval(node.left), safe_eval(node.right))
    raise ValueError(f"unsafe expression: {type(node).__name__}")


def calculate_expression(expression: str) -> str:
    """Safely evaluate a mathematical expression. Only arithmetic allowed."""
    if not expression:
        return "error: expression is required"
    try:
        tree = ast.parse(expression.strip(), mode="eval")
        result = safe_eval(tree.body)
        return str(result)
    except (SyntaxError, ValueError, ZeroDivisionError) as exc:
        return f"calculate error: {exc}"
