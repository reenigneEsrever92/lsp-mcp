#pragma once

#include <string>

namespace greeter {

/// A person we can greet.
struct Person {
    /// The person's display name.
    std::string name;
    /// The person's age in whole years.
    int age = 0;
};

/// Returns a greeting addressed to `person`.
std::string greet(const Person& person);

/// Returns a greeting for `name`.
std::string greet(const std::string& name);

}  // namespace greeter
