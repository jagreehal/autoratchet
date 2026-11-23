from typing import cast


class User:
    id: str


def as_user(raw: User | None) -> User:
    return cast(User, raw)


def as_user_checked(raw: User | None) -> User:
    assert raw is not None
    # SAFETY: the assert above rules out None.
    return cast(User, raw)


class Money:
    def cast(self, currency: str) -> "Money":
        return self


def in_pounds(money: Money) -> Money:
    return money.cast("GBP")


def apply(cast: type[str], raw: str) -> str:
    return cast(raw)
