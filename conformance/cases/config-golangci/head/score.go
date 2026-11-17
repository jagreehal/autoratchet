package score

func Score(x [8]bool) int {
	n := 0
	if x[0] {
		n++
	}
	if x[1] {
		n++
	}
	if x[2] {
		n++
	}
	if x[3] {
		n++
	}
	if x[4] {
		n++
	}
	if x[5] {
		n++
	}
	if x[6] {
		n++
	}
	if x[7] {
		n++
	}
	return n
}
