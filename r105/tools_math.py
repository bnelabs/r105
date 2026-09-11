"""Math helpers for tools — safe arithmetic evaluation and unit conversion.

Canonical implementation backing the ``calculate`` and ``convert`` tools
(``r105/tools.py`` delegates here so there is exactly one safe evaluator).
No registry imports.
"""

from __future__ import annotations

import ast
import math
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

# Whitelisted pure functions (positional args only, no attribute access).
_SAFE_FUNCTIONS: dict[str, Any] = {
    "sqrt": math.sqrt,
    "sin": math.sin,
    "cos": math.cos,
    "tan": math.tan,
    "asin": math.asin,
    "acos": math.acos,
    "atan": math.atan,
    "log": math.log,
    "log10": math.log10,
    "log2": math.log2,
    "exp": math.exp,
    "floor": math.floor,
    "ceil": math.ceil,
    "abs": abs,
    "round": round,
    "min": min,
    "max": max,
    "sum": lambda *args: sum(args),
    "pow": pow,
    "degrees": math.degrees,
    "radians": math.radians,
    "factorial": math.factorial,
    "gcd": math.gcd,
}

_SAFE_CONSTANTS: dict[str, float] = {
    "pi": math.pi,
    "e": math.e,
    "tau": math.tau,
}

_MAX_EXPR_CHARS = 2000
_MAX_AST_NODES = 256
_MAX_AST_DEPTH = 64
_MAX_ABS_NUMBER = 10**100
_MAX_POWER_EXPONENT = 100
_MAX_FACTORIAL_ARGUMENT = 10_000


def _bounded_number(value: Any) -> int | float:
    """Reject non-finite or unreasonably large numeric intermediates."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"non-numeric result: {value!r}")
    if isinstance(value, float) and not math.isfinite(value):
        raise ValueError("non-finite numeric result")
    if abs(value) > _MAX_ABS_NUMBER:
        raise ValueError("numeric result exceeds 10**100")
    return value


def _check_power_operands(base: Any, exponent: Any) -> None:
    """Reject exponentiation that could create a large CPU or memory spike."""
    if not isinstance(exponent, (int, float)) or isinstance(exponent, bool):
        raise ValueError("power exponent must be numeric")
    if not math.isfinite(float(exponent)):
        raise ValueError("power exponent must be finite")
    if abs(exponent) > _MAX_POWER_EXPONENT:
        raise ValueError(
            f"power exponent magnitude too large (max {_MAX_POWER_EXPONENT})"
        )
    if (
        isinstance(base, (int, float))
        and not isinstance(base, bool)
        and not math.isfinite(float(base))
    ):
        raise ValueError("power base must be finite")


def safe_eval(node: ast.AST, *, _depth: int = 0) -> Any:
    """Recursively evaluate a safe AST expression.

    Allowed: numeric literals, whitelisted operators, whitelisted ``math``
    functions (calls by name only), and ``pi``/``e``/``tau`` constants.
    No attribute access, subscripts, lambdas, or comprehensions.
    """
    if _depth > _MAX_AST_DEPTH:
        raise ValueError(f"expression nesting exceeds {_MAX_AST_DEPTH} levels")
    if isinstance(node, ast.Constant):
        if isinstance(node.value, bool) or not isinstance(node.value, (int, float)):
            raise ValueError(f"unsafe constant: {node.value!r}")
        return _bounded_number(node.value)
    if isinstance(node, ast.Name):
        if node.id in _SAFE_CONSTANTS:
            return _SAFE_CONSTANTS[node.id]
        raise ValueError(f"unknown name: {node.id!r}")
    if isinstance(node, ast.Call):
        if not isinstance(node.func, ast.Name):
            raise ValueError("only direct function calls are allowed")
        func = _SAFE_FUNCTIONS.get(node.func.id)
        if func is None:
            raise ValueError(f"unsafe function: {node.func.id!r}")
        if node.keywords:
            raise ValueError("keyword arguments are not allowed")
        if any(isinstance(a, ast.Starred) for a in node.args):
            raise ValueError("starred arguments are not allowed")
        values = [safe_eval(arg, _depth=_depth + 1) for arg in node.args]
        if node.func.id == "factorial":
            if len(values) != 1 or not isinstance(values[0], int) or isinstance(values[0], bool):
                raise ValueError("factorial requires one integer argument")
            if values[0] < 0:
                raise ValueError("factorial requires a non-negative argument")
            if values[0] > _MAX_FACTORIAL_ARGUMENT:
                raise ValueError(
                    f"factorial argument too large (max {_MAX_FACTORIAL_ARGUMENT})"
                )
        elif node.func.id == "pow":
            if len(values) not in (2, 3):
                raise ValueError("pow requires two or three arguments")
            _check_power_operands(values[0], values[1])
        return _bounded_number(func(*values))
    if isinstance(node, ast.UnaryOp):
        op = _SAFE_OPS.get(type(node.op))
        if op is None:
            raise ValueError(f"unsafe operator: {type(node.op).__name__}")
        return _bounded_number(op(safe_eval(node.operand, _depth=_depth + 1)))
    if isinstance(node, ast.BinOp):
        op = _SAFE_OPS.get(type(node.op))
        if op is None:
            raise ValueError(f"unsafe operator: {type(node.op).__name__}")
        left = safe_eval(node.left, _depth=_depth + 1)
        right = safe_eval(node.right, _depth=_depth + 1)
        if op is operator.pow:
            _check_power_operands(left, right)
        return _bounded_number(op(left, right))
    raise ValueError(f"unsafe expression: {type(node).__name__}")


def calculate_expression(expression: str) -> str:
    """Safely evaluate a mathematical expression.

    Supports arithmetic, parentheses, whitelisted ``math`` functions
    (sqrt, sin, cos, log, ...) and the constants pi/e/tau.
    """
    if not expression:
        return "error: expression is required"
    text = expression.strip()
    if len(text) > _MAX_EXPR_CHARS:
        return f"calculate error: expression too long (max {_MAX_EXPR_CHARS} chars)"
    try:
        tree = ast.parse(text, mode="eval")
        if sum(1 for _ in ast.walk(tree)) > _MAX_AST_NODES:
            return f"calculate error: expression is too complex (max {_MAX_AST_NODES} AST nodes)"
        result = safe_eval(tree.body)
        return str(result)
    except (
        MemoryError,
        RecursionError,
        SyntaxError,
        TypeError,
        ValueError,
        ZeroDivisionError,
        OverflowError,
    ) as exc:
        return f"calculate error: {exc}"


# -- Unit conversion --------------------------------------------------------

# Linear units: alias -> factor in the category's base unit.
_length: dict[str, float] = {
    "m": 1.0, "meter": 1.0, "meters": 1.0, "metre": 1.0, "metres": 1.0,
    "km": 1000.0, "kilometer": 1000.0, "kilometers": 1000.0,
    "cm": 0.01, "centimeter": 0.01, "centimeters": 0.01,
    "mm": 0.001, "millimeter": 0.001, "millimeters": 0.001,
    "mi": 1609.344, "mile": 1609.344, "miles": 1609.344,
    "yd": 0.9144, "yard": 0.9144, "yards": 0.9144,
    "ft": 0.3048, "foot": 0.3048, "feet": 0.3048,
    "in": 0.0254, "inch": 0.0254, "inches": 0.0254,
}

_mass: dict[str, float] = {
    "kg": 1.0, "kilogram": 1.0, "kilograms": 1.0,
    "g": 0.001, "gram": 0.001, "grams": 0.001,
    "mg": 1e-6, "milligram": 1e-6, "milligrams": 1e-6,
    "lb": 0.45359237, "lbs": 0.45359237, "pound": 0.45359237, "pounds": 0.45359237,
    "oz": 0.028349523125, "ounce": 0.028349523125, "ounces": 0.028349523125,
    "t": 1000.0, "tonne": 1000.0, "tonnes": 1000.0,
}

_time: dict[str, float] = {
    "s": 1.0, "sec": 1.0, "secs": 1.0, "second": 1.0, "seconds": 1.0,
    "ms": 0.001, "millisecond": 0.001, "milliseconds": 0.001,
    "min": 60.0, "minute": 60.0, "minutes": 60.0,
    "h": 3600.0, "hr": 3600.0, "hrs": 3600.0, "hour": 3600.0, "hours": 3600.0,
    "day": 86400.0, "days": 86400.0,
    "week": 604800.0, "weeks": 604800.0,
}

_data: dict[str, float] = {
    "b": 1.0, "byte": 1.0, "bytes": 1.0,
    "kb": 1000.0, "mb": 1000.0**2, "gb": 1000.0**3, "tb": 1000.0**4,
    "kib": 1024.0, "mib": 1024.0**2, "gib": 1024.0**3, "tib": 1024.0**4,
    "bit": 0.125, "bits": 0.125,
    "kbit": 125.0, "mbit": 125000.0, "gbit": 125000000.0,
}

_speed: dict[str, float] = {
    "m/s": 1.0, "mps": 1.0,
    "kph": 1000.0 / 3600.0, "km/h": 1000.0 / 3600.0, "kmh": 1000.0 / 3600.0,
    "mph": 1609.344 / 3600.0,
    "knot": 1852.0 / 3600.0, "knots": 1852.0 / 3600.0, "kt": 1852.0 / 3600.0,
    "ft/s": 0.3048, "fps": 0.3048,
}

_volume: dict[str, float] = {
    "l": 1.0, "liter": 1.0, "liters": 1.0, "litre": 1.0, "litres": 1.0,
    "ml": 0.001, "milliliter": 0.001, "milliliters": 0.001,
    "gal": 3.785411784, "gallon": 3.785411784, "gallons": 3.785411784,
    "cup": 0.2365882365, "cups": 0.2365882365,
    "floz": 0.0295735295625, "fl-oz": 0.0295735295625,
}

_LINEAR_CATEGORIES: dict[str, dict[str, float]] = {
    "length": _length,
    "mass": _mass,
    "time": _time,
    "data": _data,
    "speed": _speed,
    "volume": _volume,
}

_temperature = {"c", "celsius", "f", "fahrenheit", "k", "kelvin"}


def _to_celsius(value: float, unit: str) -> float:
    if unit in {"c", "celsius"}:
        return value
    if unit in {"f", "fahrenheit"}:
        return (value - 32.0) * 5.0 / 9.0
    return value - 273.15  # kelvin


def _from_celsius(value: float, unit: str) -> float:
    if unit in {"c", "celsius"}:
        return value
    if unit in {"f", "fahrenheit"}:
        return value * 9.0 / 5.0 + 32.0
    return value + 273.15  # kelvin


def convert_units(value: float, from_unit: str, to_unit: str) -> str:
    """Convert *value* from *from_unit* to *to_unit*.

    Categories: length, mass, time, data, speed, volume, temperature.
    Returns a human string like ``"1.0 km = 1000.0 m"`` or an error string.
    """
    src = from_unit.strip().lower()
    dst = to_unit.strip().lower()
    if src in _temperature or dst in _temperature:
        if src not in _temperature or dst not in _temperature:
            return (
                f"convert error: cannot convert between temperature "
                f"and non-temperature units ({from_unit} -> {to_unit})"
            )
        result = _from_celsius(_to_celsius(value, src), dst)
        return f"{value} {from_unit} = {result} {to_unit}"
    for category, table in _LINEAR_CATEGORIES.items():
        if src in table and dst in table:
            base_value = value * table[src]
            result = base_value / table[dst]
            return f"{value} {from_unit} = {result} {to_unit}"
        if src in table or dst in table:
            return (
                f"convert error: incompatible units for {category} "
                f"({from_unit} -> {to_unit})"
            )
    return f"convert error: unknown unit(s): {from_unit!r} and/or {to_unit!r}"
