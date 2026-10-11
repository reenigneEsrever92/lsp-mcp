package main

import (
	"fmt"

	"example.com/greeter/greeter"
)

func main() {
	person := greeter.New("Ada", 36)

	fmt.Println(greeter.Greet("world"))
	fmt.Println(person.Greet())

	// Kept deliberately so `diagnostics` has an error to report: Go treats an
	// unused local variable as a compile error ("declared and not used").
	var unused int
}
