from typing import Any, TypeGuard


class User:
    id: str


def handle(payload: Any, *args: Any, **kwargs: Any) -> None:
    return None


def is_user(value: object) -> TypeGuard[User]:
    return isinstance(value, User)


class TypeGuardResult:
    ok: bool


def check(value: Any) -> TypeGuardResult:
    return TypeGuardResult()
