package orders

type Order struct {
	Items []int
	VIP   bool
}

func (o *Order) Total() int {
	total := 0
	for _, item := range o.Items {
		total += item
	}
	return total
}

func Route(order *Order, express bool) string {
	if express {
		return "express"
	}
	if order.VIP {
		return "vip"
	}
	return "standard"
}
