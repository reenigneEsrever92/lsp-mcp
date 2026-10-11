#include <iostream>

#include "greeter.hpp"

int main() {
    const greeter::Person person{"Ada", 36};

    std::cout << greeter::greet("world") << '\n';
    std::cout << greeter::greet(person) << '\n';

    // Kept deliberately unused so `diagnostics` has a warning to report
    // (clang's -Wunused-variable, enabled below).
    int unused = 0;

    return 0;
}
