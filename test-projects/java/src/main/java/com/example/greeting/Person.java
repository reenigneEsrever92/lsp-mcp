package com.example.greeting;

/**
 * A person with a name and an age.
 */
public class Person {
    private final String name;
    private final int age;

    /**
     * Builds a {@code Person} with the given name and age.
     *
     * @param name the person's name
     * @param age  the person's age in years
     */
    public Person(String name, int age) {
        this.name = name;
        this.age = age;
    }

    /**
     * Greets this person by name.
     *
     * @return a greeting addressed to this person
     */
    public String greet() {
        return "Hello, " + name + "!";
    }

    /**
     * Returns this person's name.
     *
     * @return the name
     */
    public String getName() {
        return name;
    }

    /**
     * Returns this person's age.
     *
     * @return the age in years
     */
    public int getAge() {
        return age;
    }
}
