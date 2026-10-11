use greeter::{greet, Person};

fn main() {
    let person = Person::new("Ada", 36);

    println!("{}", greet("world"));
    println!("{}", person.greet());

    // Kept deliberately unused so `diagnostics` has a warning to report
    // (rustc's `unused_variables` lint).
    let unused = 0;
}
