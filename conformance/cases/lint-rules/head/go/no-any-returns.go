package fixtures

func Load(raw string) interface{} {
	return raw
}

func Named() (out any) {
	return nil
}

func Both() (any, interface{}) {
	return nil, nil
}

func Grouped() (a, b any) {
	return nil, nil
}

type Callable func() any

type Runner interface {
	Run() any
}
