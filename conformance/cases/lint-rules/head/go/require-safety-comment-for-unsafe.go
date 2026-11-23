package fixtures

import "unsafe"

func Size(x int64) uintptr {
	return unsafe.Sizeof(x)
}

func SizeChecked(x int64) uintptr {
	// SAFETY: Sizeof only reads the static type; nothing is dereferenced.
	return unsafe.Sizeof(x)
}
