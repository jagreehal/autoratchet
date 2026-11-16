from typing import overload


class Order:
    def __init__(self, items, vip=False):
        self.items = items
        self.vip = vip

    @property
    def total(self):
        return sum(self.items)

    @total.setter
    def total(self, value):
        raise AttributeError("total is computed")


def route(order, express, member, country):
    if express:
        if member:
            if country == "GB":
                return "express-member-gb"
            elif country == "FR" or country == "DE":
                return "express-member-eu"
            return "express-member"
        for item in order.items:
            if item > 100 and order.vip:
                return "express-heavy-vip"
        return "express"
    match country:
        case "GB" if order.vip:
            return "vip-gb"
        case "GB":
            return "gb"
        case _:
            return "standard" if order.items else "empty"


def status(code):  # noqa: C901 -- one branch per status code, no logic
    if code == 200: return "ok"
    if code == 201: return "created"
    if code == 204: return "no content"
    if code == 301: return "moved"
    if code == 304: return "not modified"
    if code == 400: return "bad request"
    if code == 401: return "unauthorised"
    if code == 403: return "forbidden"
    if code == 404: return "not found"
    if code == 500: return "error"
    return "unknown"


# complexipy: ignore -- follows the parsing steps of the spec in order
def parse(text):
    parts = text.split(".")
    if parts:
        for part in parts:
            if part:
                while len(part) > 100:
                    if part.startswith("v") and part.endswith("x") or not part:
                        return None
    return parts


@overload
def first(values: list[int]) -> int: ...
@overload
def first(values: list[str]) -> str: ...
def first(values):
    return values[0] if values else None


def walk(node):
    return [walk(child) for child in node.children] if node.children else []
