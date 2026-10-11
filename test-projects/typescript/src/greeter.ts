/** A person we can greet. */
export interface Person {
  /** The person's display name. */
  name: string;
  /** The person's age in whole years. */
  age: number;
}

/** Builds a {@link Person} from a name and an age. */
export function person(name: string, age: number): Person {
  return { name, age };
}

/** Returns a greeting addressed to a {@link Person} or a bare name. */
export function greet(personOrName: Person | string): string {
  const name = typeof personOrName === "string" ? personOrName : personOrName.name;
  return `Hello, ${name}!`;
}
