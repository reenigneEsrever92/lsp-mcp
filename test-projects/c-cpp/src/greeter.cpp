#include "greeter.hpp"

namespace greeter {

std::string greet(const Person& person) {
    return greet(person.name);
}

std::string greet(const std::string& name) {
    return "Hello, " + name + "!";
}

}  // namespace greeter
