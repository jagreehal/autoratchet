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


def route(order, express):
    if express:
        return "express"
    if order.vip:
        return "vip"
    return "standard"
