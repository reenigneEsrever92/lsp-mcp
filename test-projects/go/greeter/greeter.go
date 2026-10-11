// Package greeter builds greetings for the Go test project.
//
// main.go imports this package, so the cross-file reference to Person gives
// definition, references, hover, workspaceSymbol, and rename something to act on.
package greeter

import "fmt"

// Person is someone we can greet.
type Person struct {
	// Name is the person's display name.
	Name string
	// Age is the person's age in whole years.
	Age int
}

// New builds a Person from a name and an age.
func New(name string, age int) Person {
	return Person{Name: name, Age: age}
}

// Greet returns a greeting addressed to the person.
func (p Person) Greet() string {
	return Greet(p.Name)
}

// Greet returns a greeting for name.
func Greet(name string) string {
	return fmt.Sprintf("Hello, %s!", name)
}
