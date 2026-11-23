package fixtures

import "fmt"

func Message(v fmt.Stringer) string {
	err := v.(error)
	if checked, ok := v.(error); ok {
		return checked.Error()
	}
	switch t := v.(type) {
	case error:
		return t.Error()
	}
	return err.Error()
}

func Parenthesized(v fmt.Stringer) bool {
	_, ok := (v.(error))
	return ok
}

func Paired(v fmt.Stringer) int {
	a, b := v.(error), 2
	_ = a
	return b
}
