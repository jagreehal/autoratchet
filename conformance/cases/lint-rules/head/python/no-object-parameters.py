def save(value: object) -> None:
    return None


def local_import(raw: str) -> str:
    from typing import cast

    # SAFETY: raw is already a str; the cast documents it for the checker.
    return cast(str, raw)


def elsewhere(raw: str) -> str:
    return cast(raw)


class Point:
    x: int

    def __eq__(self, other: object) -> bool:
        return isinstance(other, Point) and other.x == self.x


NO_DEFAULT = object()


def field(name: str, default: object = NO_DEFAULT, fallback: object = None) -> str:
    return name if default is NO_DEFAULT else str(fallback)
