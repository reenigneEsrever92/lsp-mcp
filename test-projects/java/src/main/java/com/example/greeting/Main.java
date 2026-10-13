package com.example.greeting;

/**
 * Demonstrates the greeting library.
 */
public class Main {
    public static void main(String[] args) {
        Person person = new Person("Ada", 36);
        System.out.println(person.greet());
        // Deliberate diagnostic: the statement below is missing its `;`.
        System.out.println(person.getName());
    }
}
