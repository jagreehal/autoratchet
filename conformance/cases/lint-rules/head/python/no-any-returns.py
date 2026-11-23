from typing import Any


def load(raw: str) -> Any:
    return raw


def cast(value: int) -> int:
    return value


def doubled(value: int) -> int:
    return cast(value) * 2
