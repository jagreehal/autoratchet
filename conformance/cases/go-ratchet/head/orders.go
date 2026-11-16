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

func Route(order *Order, express, member bool, country string) string {
	if express {
		if member {
			if country == "GB" {
				return "express-member-gb"
			} else if country == "FR" || country == "DE" {
				return "express-member-eu"
			}
			return "express-member"
		}
		for _, item := range order.Items {
			if item > 100 && order.VIP {
				return "express-heavy-vip"
			}
		}
		return "express"
	}
	switch {
	case country == "GB" && order.VIP:
		return "vip-gb"
	case country == "GB":
		return "gb"
	default:
		return "standard"
	}
}

//nolint:gocyclo // one case per status code, no logic
func Status(code int) string {
	switch code {
	case 200:
		return "ok"
	case 201:
		return "created"
	case 204:
		return "no content"
	case 301:
		return "moved"
	case 304:
		return "not modified"
	case 400:
		return "bad request"
	case 401:
		return "unauthorised"
	case 403:
		return "forbidden"
	case 404:
		return "not found"
	case 500:
		return "error"
	}
	return "unknown"
}

//gocognit:ignore follows the parsing steps of the spec in order
func Parse(parts []string) []string {
	if len(parts) > 0 {
		for _, part := range parts {
			if part != "" {
				for len(part) > 100 {
					if part[0] == 'v' && part[len(part)-1] == 'x' || part == "" {
						return nil
					}
				}
			}
		}
	}
	return parts
}

var Handler = func(code int) string {
	if code > 0 {
		return "positive"
	}
	return "other"
}

func Walk(depth int) int {
	if depth == 0 {
		return 0
	}
	return Walk(depth - 1)
}
