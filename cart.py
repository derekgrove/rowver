"""Compatibility shim: the press library lives in press.py now.

Study recipes written before 2026-08-28 import ``from cart import Cart``;
they keep working through this line. New recipes import from ``press``.
"""
from press import *          # noqa: F401,F403
from press import Cart, CartError, study_names   # noqa: F401
