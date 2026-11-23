package fixtures

import "fmt"

func Handle(v any) {
	fmt.Println(v)
}

func Logf(format string, args ...any) {
	fmt.Printf(format, args...)
}
